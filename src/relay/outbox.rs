//! Notices other programs post in the DM through the relay, as
//! `switchboard slack-relay notify` (lib/slack-relay/outbox.js): a loop
//! that has to reach you, such as the oncall skill. They go out as the
//! app's bot, so Slack notifies you of them, unlike anything posted as you,
//! which it never does. Each one is a file in the outbox, which the running
//! relay posts and answers with `<id>.sent` (the message's ts) or
//! `<id>.failed` (why), which `notify` waits for, so a caller knows whether
//! you were reached.
//!
//! What it posts is only text, in the DM, like everything the relay writes,
//! and at once, whether or not you are at the computer: a notice is for you
//! to see now.

use std::path::Path;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use regex::Regex;
use serde_json::{Value, json};

use super::Relay;
use super::config::Paths;
use super::post::{chunks, post_one};
use crate::util;

/// How long `notify` waits for the relay to post, which it does within a
/// second unless Slack is slow to answer.
pub const NOTIFY_WAIT: Duration = Duration::from_secs(45);
/// Answers nobody collected, and notices left by a relay that stopped
/// before posting them, are dropped after this long: a notice an hour late
/// is noise.
pub const OUTBOX_KEEP_MS: i64 = 60 * 60_000;

/// A short name for who sent it, shown above the notice.
static FROM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[\w .:-]{1,40}$").expect("a regex"));

/// Queues `text` and waits for the relay to post it: the message's ts, or
/// why it wasn't posted.
pub fn notify(paths: &Paths, text: &str, from: Option<&str>, wait: Duration) -> Result<String> {
    let from = from.unwrap_or("notice");
    if text.trim().is_empty() {
        bail!("nothing to send");
    }
    if !FROM.is_match(from) {
        bail!("--from takes up to 40 letters, digits, spaces and . : _ -");
    }
    if super::lifecycle::lock_holder(paths).is_none() {
        bail!("the relay isn't running; start it with `switchboard slack-relay`");
    }
    let dir = paths.outbox_dir();
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)?;
    }
    let random: String = crate::tabs::color::sha256(
        format!(
            "{}-{}-{:?}",
            util::now_ms(),
            std::process::id(),
            Instant::now()
        )
        .as_bytes(),
    )[..3]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let id = format!("{}-{}-{random}", util::now_ms(), std::process::id());
    let file = dir.join(&id);
    // Written whole, then renamed into place, so the relay never reads half.
    let tmp = dir.join(format!("{id}.tmp"));
    util::write_private(
        &tmp,
        util::json_compact(&json!({ "from": from, "text": text })),
        false,
    )?;
    std::fs::rename(&tmp, dir.join(format!("{id}.json")))?;
    let until = Instant::now() + wait;
    let answer = |ending: &str| dir.join(format!("{id}{ending}"));
    while Instant::now() < until {
        for (ending, ok) in [(".sent", true), (".failed", false)] {
            let Some(said) = util::read_text(answer(ending)) else {
                continue;
            };
            let _ = std::fs::remove_file(answer(ending));
            return if ok { Ok(said) } else { Err(anyhow!("{said}")) };
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    // Not posted in time: taken back, so it can't arrive late as well.
    let _ = std::fs::remove_file(file.with_extension("json"));
    let _ = std::fs::remove_file(dir.join(format!("{id}.json")));
    bail!("the relay didn't post it in time")
}

/// When a notice's file was made, from its name.
fn made_at(name: &str) -> i64 {
    name.split('-')
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

/// In the relay: posts every queued notice, oldest first, and answers each.
pub async fn deliver_outbox(cx: &Relay) {
    let dir = cx.paths.outbox_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let now = util::now_ms();
    for name in names.iter().filter(|n| now - made_at(n) > OUTBOX_KEEP_MS) {
        let _ = std::fs::remove_file(dir.join(name));
    }
    for name in names
        .iter()
        .filter(|n| n.ends_with(".json") && now - made_at(n) <= OUTBOX_KEEP_MS)
    {
        let file = dir.join(name);
        let id = name.trim_end_matches(".json");
        let Some(notice) = util::read_json(&file) else {
            continue;
        };
        if std::fs::remove_file(&file).is_err() {
            continue;
        }
        let from = notice
            .get("from")
            .and_then(Value::as_str)
            .filter(|f| FROM.is_match(f))
            .unwrap_or("notice")
            .to_string();
        let text = notice.get("text").map(util::js_string).unwrap_or_default();
        let posted: Result<String> = async {
            let parts = chunks(&format!("🔔 **{from}**\n{text}"));
            let ts = post_one(cx, &parts[0], None, false, &[]).await?;
            for part in &parts[1..] {
                post_one(cx, part, Some(&ts), false, &[]).await?;
            }
            Ok(ts)
        }
        .await;
        match posted {
            Ok(ts) => {
                let _ = util::write_private(answer(&dir, id, ".sent"), ts, false);
                cx.log(&format!("posted a notice from {from}"));
            }
            Err(e) => {
                let _ = util::write_private(answer(&dir, id, ".failed"), e.to_string(), false);
                cx.log(&format!("notice failed: {e}"));
            }
        }
    }
}

fn answer(dir: &Path, id: &str, ending: &str) -> std::path::PathBuf {
    dir.join(format!("{id}{ending}"))
}
