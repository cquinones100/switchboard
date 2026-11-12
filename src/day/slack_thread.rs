//! The Slack view's threads and their pictures, new with UX 2.0's board 21:
//! the chosen message's whole thread beside the list, with its pictures and
//! videos' first frames. Slack's search answers with each message alone, no
//! reply count and no files, so both come from reading the thread
//! (`slack_read_thread`), which names each message's files: the server's
//! look reads the threads its messages are in whose newest message is newer
//! than what it keeps (`wanted`, at most `PER_LOOK` a look), the
//! /slack-updates skill passes the threads it reads to its save, and
//! choosing a message whose thread isn't kept reads it at once.
//!
//! What was read is kept in `slack-threads.json` in the state directory,
//! by the thread's key (`slack::Message::thread_key`), only for threads of
//! messages still in `slack.json`. A picture or a video of the chosen
//! thread is read through `slack_read_file` once and kept in
//! `~/.cache/switchboard/slack-files/<file id>` for `FILES_KEEP`, a video's
//! first frame beside it (`platform::video_frame`), and drawn from there,
//! shrunk to `SHOWN` pixels at most. Nothing is read on the drawing path:
//! the server reads them in tasks of its own (src/day/serve.rs) and keeps
//! the decoded pictures for mission control.
//!
//! Turned off with Settings' `slack_threads`, which reads none of it.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime};

use image::RgbaImage;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Connector, slack};
use crate::{paths, util};

/// The most threads a look reads.
pub const PER_LOOK: usize = 8;
/// How old a chosen thread may be before choosing it reads it again.
pub const STALE_MS: i64 = 5 * 60_000;
/// The largest file kept, as the read-file tool reads at most.
const LARGEST_FILE: usize = 10 * 1024 * 1024;
/// How many pages of a thread are read at most, each of `PAGE` messages
/// (the tool's most), so any thread is read whole.
const THREAD_PAGES: usize = 5;
const PAGE: u32 = 1000;
/// How long a file read is kept.
const FILES_KEEP: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// The widest or tallest a picture is held for drawing, in pixels.
const SHOWN: u32 = 512;
const SLACK: &str = "mcp__claude_ai_Slack__";

/// A file attached to a message, as the thread names it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlackFile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub mimetype: String,
}

/// The kinds of file the board keeps and draws, by the type the thread
/// names: the extension it is kept under, whether it is a video, and how
/// its first bytes start. A file is kept only when its bytes are that kind
/// (`sniffs_as`), and only ever shown with Quick Look, never opened, since
/// a message's text can claim any name and type for a file.
const KINDS: &[(&str, &str, bool)] = &[
    ("image/png", "png", false),
    ("image/jpeg", "jpg", false),
    ("image/gif", "gif", false),
    ("image/webp", "webp", false),
    ("video/mp4", "mp4", true),
    ("video/quicktime", "mov", true),
];

impl SlackFile {
    fn kind(&self) -> Option<&'static (&'static str, &'static str, bool)> {
        KINDS
            .iter()
            .find(|(t, _, _)| self.mimetype.eq_ignore_ascii_case(t))
    }

    pub fn is_picture(&self) -> bool {
        self.kind().is_some_and(|k| !k.2)
    }

    pub fn is_video(&self) -> bool {
        self.kind().is_some_and(|k| k.2)
    }

    /// A picture or a video of a kind the board keeps, which it draws.
    pub fn drawn(&self) -> bool {
        self.kind().is_some()
    }
}

/// Whether `bytes` start as a file of the extension `ext` does.
fn sniffs_as(bytes: &[u8], ext: &str) -> bool {
    let at = |from: usize, magic: &[u8]| bytes.get(from..from + magic.len()) == Some(magic);
    match ext {
        "png" => at(0, b"\x89PNG\r\n\x1a\n"),
        "jpg" => at(0, b"\xff\xd8\xff"),
        "gif" => at(0, b"GIF87a") || at(0, b"GIF89a"),
        "webp" => at(0, b"RIFF") && at(8, b"WEBP"),
        "mp4" | "mov" => [&b"ftyp"[..], b"moov", b"mdat", b"wide", b"free"]
            .iter()
            .any(|m| at(4, m)),
        _ => false,
    }
}

/// One message of a thread.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Said {
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub text: String,
    pub ts: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<SlackFile>,
    /// The ticket IDs it names, its links' included.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<String>,
}

/// A thread as read: its first message and the replies, oldest first, how
/// many replies Slack counts, and when it was read (milliseconds).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Thread {
    pub messages: Vec<Said>,
    #[serde(default)]
    pub replies: u32,
    #[serde(default)]
    pub read: i64,
}

impl Thread {
    fn newest(&self) -> f64 {
        self.messages
            .iter()
            .filter_map(|m| m.ts.parse::<f64>().ok())
            .fold(0.0, f64::max)
    }

    /// Its pictures and videos, in order.
    pub fn files(&self) -> impl Iterator<Item = &SlackFile> {
        self.messages
            .iter()
            .flat_map(|m| m.files.iter())
            .filter(|f| f.drawn())
    }
}

pub fn file(state: &Path) -> PathBuf {
    state.join("slack-threads.json")
}

/// The threads kept, by their key.
pub fn read(state: &Path) -> BTreeMap<String, Thread> {
    util::read_json_object(file(state))
        .into_iter()
        .filter_map(|(k, v)| Some((k, serde_json::from_value(v).ok()?)))
        .collect()
}

/// Keeps the threads read, by their key, with those kept before, letting go
/// of any whose messages have left `slack.json`; with none read, only lets
/// go, writing nothing where nothing was kept.
pub fn save(state: &Path, read_now: BTreeMap<String, Thread>) -> std::io::Result<()> {
    // The server's two readers (the look's and the chosen thread's) save
    // one at a time, so neither writes over what the other just kept.
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if read_now.is_empty() && !file(state).exists() {
        return Ok(());
    }
    let keys: HashSet<String> = slack::read(state)
        .map(|s| s.items.iter().map(slack::Message::thread_key).collect())
        .unwrap_or_default();
    let mut all = read(state);
    all.extend(read_now);
    all.retain(|k, _| keys.contains(k));
    util::write_atomic(file(state), util::json_compact(&all))
}

/// The threads a look should read: those its saved messages are replies
/// in, or kept ones you have written in, whose newest message is newer
/// than the thread kept, newest first, at most `most`. A message in no
/// thread is read when it is chosen, and a chosen thread read over
/// `STALE_MS` ago again then, for replies no look saves.
pub fn wanted(
    saved: &slack::Saved,
    kept: &BTreeMap<String, Thread>,
    most: usize,
) -> Vec<(String, String)> {
    let mut newest: BTreeMap<String, (f64, String, String)> = BTreeMap::new();
    for m in &saved.items {
        let Some(thread) = m.thread.as_deref().filter(|t| !t.is_empty()) else {
            continue;
        };
        let ts: f64 = m.ts.parse().unwrap_or(0.0);
        let at =
            newest
                .entry(m.thread_key())
                .or_insert((0.0, m.channel.clone(), thread.to_string()));
        at.0 = at.0.max(ts);
    }
    // A kept thread you have written in since, which the saved messages
    // leave out.
    for (k, v) in &saved.replied {
        let Some((channel, thread)) = k.split_once(':') else {
            continue;
        };
        if !kept.contains_key(k) {
            continue;
        }
        let ts: f64 = util::js_string(v).parse().unwrap_or(0.0);
        let at = newest
            .entry(k.clone())
            .or_insert((0.0, channel.to_string(), thread.to_string()));
        at.0 = at.0.max(ts);
    }
    let mut out: Vec<(f64, String, String)> = newest
        .into_iter()
        .filter(|(k, (ts, _, _))| kept.get(k).is_none_or(|t| t.newest() < *ts))
        .map(|(_, v)| v)
        .collect();
    out.sort_by(|a, b| b.0.total_cmp(&a.0));
    out.into_iter().take(most).map(|(_, c, t)| (c, t)).collect()
}

static BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^(=== THREAD PARENT MESSAGE ===|--- Reply \d+ of \d+ ---)[ \t]*\r?$").unwrap()
});
static REPLIES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^=== THREAD REPLIES \((\d+) total\) ===").unwrap());
static FROM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(.*?)(?: <[^>]*>)? \((U\w+|B\w+|W\w+)\)\s*$").unwrap());
static FILE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(.+?) \(ID: (F\w+), ([^,()]+), [^)]*\)(?:, |$)").unwrap());

/// A thread from `slack_read_thread`'s text:
///
/// ```text
/// === THREAD PARENT MESSAGE ===
/// From: Ada <ada@example.com> (U1)
/// Time: 2026-11-10 13:30:12 EST
/// Message TS: 1794335412.484889
/// the message's text
/// Files: shot.png (ID: F1, image/png, 48.2 KB), clip.mov (ID: F2, video/quicktime, 2 MB)
///
/// === THREAD REPLIES (3 total) ===
///
/// --- Reply 1 of 3 ---
/// From: …
/// ```
///
/// `None` when it holds no message.
pub fn thread_from(text: &str, now: i64) -> Option<Thread> {
    let heads: Vec<(usize, usize)> = BLOCK
        .find_iter(text)
        .map(|m| (m.start(), m.end()))
        .collect();
    let mut messages = Vec::new();
    for (i, (_, from)) in heads.iter().enumerate() {
        let to = heads.get(i + 1).map_or(text.len(), |h| h.0);
        let block = &text[*from..to];
        let block = match REPLIES.find(block) {
            Some(m) => &block[..m.start()],
            None => block,
        };
        let mut said = Said::default();
        let mut body = Vec::new();
        let mut in_body = false;
        for line in block.lines() {
            if !in_body {
                if let Some(v) = line.strip_prefix("From: ") {
                    if let Some(c) = FROM.captures(v.trim()) {
                        said.from = c[1].trim().to_string();
                        said.user = c[2].to_string();
                    } else {
                        said.from = v.trim().to_string();
                    }
                } else if let Some(v) = line.strip_prefix("Message TS: ") {
                    said.ts = v.trim().to_string();
                    in_body = true;
                }
                continue;
            }
            // The connector's list of the message's files; a line of the
            // message's own that only starts the same way stays its text.
            let listed: Vec<SlackFile> = line
                .strip_prefix("Files: ")
                .map(|list| {
                    FILE.captures_iter(list.trim())
                        .map(|c| SlackFile {
                            name: c[1].trim().to_string(),
                            id: c[2].to_string(),
                            mimetype: c[3].trim().to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            if listed.is_empty() {
                body.push(line);
            } else {
                said.files.extend(listed);
            }
        }
        let raw = body.join("\n");
        // The tickets it names, a link's included, before a link becomes its
        // words.
        said.refs = crate::views::ticket::ids_in(&raw);
        said.text = super::slack_look::plain(raw.trim());
        if said.ts.parse::<f64>().is_ok() {
            messages.push(said);
        }
    }
    if messages.is_empty() {
        return None;
    }
    let replies = REPLIES
        .captures(text)
        .and_then(|c| c[1].parse().ok())
        .unwrap_or(messages.len().saturating_sub(1) as u32);
    Some(Thread {
        messages,
        replies,
        read: now,
    })
}

/// Reads one thread through the connector.
pub async fn read_thread(
    call: &impl Connector,
    channel: &str,
    ts: &str,
    now: i64,
) -> anyhow::Result<Thread> {
    let mut whole: Option<Thread> = None;
    let mut cursor: Option<String> = None;
    // Its pages, as the search's are followed, up to `THREAD_PAGES`.
    for _ in 0..THREAD_PAGES {
        let mut input = json!({
            "channel_id": channel, "message_ts": ts, "response_format": "detailed",
            "limit": PAGE,
        });
        if let Some(c) = &cursor {
            input["cursor"] = json!(c);
        }
        let answer = call
            .call(&format!("{SLACK}slack_read_thread"), input, None)
            .await?;
        let (text, next) = super::slack_look::page_of(&answer);
        let text = if text.is_empty() { answer } else { text };
        if let Some(page) = thread_from(&text, now) {
            match whole.as_mut() {
                None => whole = Some(page),
                Some(t) => {
                    for m in page.messages {
                        if !t.messages.iter().any(|had| had.ts == m.ts) {
                            t.messages.push(m);
                        }
                    }
                    t.replies = t.replies.max(page.replies);
                }
            }
        }
        match next {
            Some(n) if Some(&n) != cursor.as_ref() => cursor = Some(n),
            _ => break,
        }
    }
    whole.ok_or_else(|| anyhow::anyhow!("no messages in the thread"))
}

/// Reads each of `threads` (channel, thread ts) and keeps those read, one
/// that fails leaving the others to be read and kept; fails only when what
/// was read couldn't be kept.
pub async fn read_threads(
    state: &Path,
    call: &impl Connector,
    threads: &[(String, String)],
    now: i64,
) -> anyhow::Result<Read> {
    let mut got = BTreeMap::new();
    let mut failed = Vec::new();
    for (channel, ts) in threads {
        let key = format!("{channel}:{ts}");
        match read_thread(call, channel, ts, now).await {
            Ok(thread) => {
                got.insert(key, thread);
            }
            Err(e) => failed.push((key, e.to_string())),
        }
    }
    let read = got.len();
    save(state, got)?;
    Ok(Read { read, failed })
}

/// What reading threads did: how many were read and kept, and why each
/// that couldn't be read couldn't, by its key.
#[derive(Debug, Default)]
pub struct Read {
    pub read: usize,
    pub failed: Vec<(String, String)>,
}

/// After a look: reads the threads it found something new in
/// (`wanted`, at most `PER_LOOK`) and keeps them, letting go of those
/// whose messages have gone. Run in a task of its own, so a slow thread
/// never holds up the next search.
/// `waiting` the keys of threads that failed lately, left for later.
pub async fn read_wanted(
    state: &Path,
    call: &impl Connector,
    waiting: &HashSet<String>,
    now: i64,
) -> anyhow::Result<Read> {
    let Some(saved) = slack::read(state) else {
        return Ok(Read::default());
    };
    let wanted: Vec<(String, String)> = wanted(&saved, &read(state), usize::MAX)
        .into_iter()
        .filter(|(c, t)| !waiting.contains(&format!("{c}:{t}")))
        .take(PER_LOOK)
        .collect();
    read_threads(state, call, &wanted, now).await
}

/// What /slack-updates passes its save: `[{ channel, thread, text }]`,
/// each `text` the read-thread tool's answer as it came.
pub fn save_found(state: &Path, threads: &[Value], now: i64) -> std::io::Result<usize> {
    let got: BTreeMap<String, Thread> = threads
        .iter()
        .filter_map(|t| {
            let channel = t.get("channel")?.as_str()?;
            let ts = t.get("thread").map(util::js_string)?;
            let answer = t.get("text")?.as_str()?;
            // The answer as it came: the text, or JSON holding it.
            let (unwrapped, more) = super::slack_look::page_of(answer);
            // A thread with pages still to read is left for the server to
            // read whole once it is chosen.
            if more.is_some() {
                return None;
            }
            let text = if unwrapped.is_empty() {
                answer
            } else {
                unwrapped.as_str()
            };
            let ok = !channel.is_empty()
                && channel.bytes().all(|b| b.is_ascii_alphanumeric())
                && ts.parse::<f64>().is_ok();
            ok.then(|| Some((format!("{channel}:{ts}"), thread_from(text, now)?)))
                .flatten()
        })
        .collect();
    let n = got.len();
    save(state, got)?;
    Ok(n)
}

// ── The files ───────────────────────────────────────────────────────────────

fn files_dir() -> PathBuf {
    paths::cache_dir().join("slack-files")
}

/// A Slack file's ID as a file's name, letters and digits alone, with the
/// extension of its kind (by which Quick Look knows a video); `None` for a
/// kind the board doesn't keep.
fn named(f: &SlackFile) -> Option<String> {
    let id = &f.id;
    if id.is_empty() || id.len() > 40 || !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    Some(format!("{id}.{}", f.kind()?.1))
}

fn fresh(path: &Path, now: SystemTime) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .is_ok_and(|at| now.duration_since(at).unwrap_or_default() < FILES_KEEP)
}

/// Where a file is kept once read, while it is fresh.
pub fn kept(f: &SlackFile) -> Option<PathBuf> {
    let path = files_dir().join(named(f)?);
    fresh(&path, SystemTime::now()).then_some(path)
}

/// Where a video's first frame is kept.
fn frame_file(f: &SlackFile) -> Option<PathBuf> {
    Some(files_dir().join(format!("{}.frame.png", named(f)?)))
}

/// The file's bytes from `slack_read_file`'s answer: an image comes as a
/// `data:` URI (`connector::call` turns an image block into one), other
/// files as base64 in the text, the longest such run.
pub fn bytes_from(answer: &str) -> Option<Vec<u8>> {
    static DATA: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"data:[\w/+.-]+;base64,([A-Za-z0-9+/=]+)").unwrap());
    static RUN: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"[A-Za-z0-9+/]{200,}={0,2}").unwrap());
    if let Some(c) = DATA.captures(answer) {
        return util::unbase64(c[1].as_bytes());
    }
    let text = serde_json::from_str::<Value>(answer)
        .ok()
        .and_then(|v| {
            ["content", "data", "base64"]
                .iter()
                .find_map(|k| v.get(*k).and_then(Value::as_str).map(str::to_string))
        })
        .unwrap_or_else(|| answer.to_string());
    let longest = RUN.find_iter(&text).max_by_key(|m| m.len())?;
    util::unbase64(longest.as_str().as_bytes())
}

/// Reads a file through the connector and keeps it, a video's first frame
/// beside it; the files kept past `FILES_KEEP` are let go first.
pub async fn fetch(call: &impl Connector, f: &SlackFile) -> anyhow::Result<PathBuf> {
    let Some(name) = named(f) else {
        anyhow::bail!("not a file ID: {}", f.id);
    };
    prune(SystemTime::now());
    let answer = call
        .call(
            &format!("{SLACK}slack_read_file"),
            json!({ "file_id": f.id }),
            None,
        )
        .await?;
    // Base64 is a third longer than what it holds.
    if answer.len() > LARGEST_FILE / 3 * 4 + 4096 {
        anyhow::bail!("{} is over 10 MB", f.name);
    }
    let bytes = bytes_from(&answer).ok_or_else(|| anyhow::anyhow!("no data for {}", f.name))?;
    if bytes.len() > LARGEST_FILE {
        anyhow::bail!("{} is over 10 MB", f.name);
    }
    let ext = f.kind().map_or("", |k| k.1);
    if !sniffs_as(&bytes, ext) {
        anyhow::bail!("{} isn't the {} it says it is", f.name, f.mimetype);
    }
    let path = files_dir().join(name);
    std::fs::create_dir_all(files_dir())?;
    util::write_atomic(&path, bytes)?;
    if f.is_video()
        && let Some(frame) = frame_file(f)
    {
        let _ = crate::platform::video_frame(&path, &frame, SHOWN);
    }
    Ok(path)
}

fn prune(now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(files_dir()) else {
        return;
    };
    for e in entries.flatten() {
        if !fresh(&e.path(), now) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// A kept picture, or a kept video's first frame, decoded and shrunk to
/// `SHOWN` pixels at most; `None` while it isn't kept or can't be drawn.
pub fn load(f: &SlackFile) -> Option<RgbaImage> {
    let path = if f.is_video() {
        kept(f)?;
        frame_file(f)?
    } else {
        kept(f)?
    };
    let img = crate::ui::picture::decode(&path).ok()?;
    Some(if img.width() > SHOWN || img.height() > SHOWN {
        image::DynamicImage::ImageRgba8(img)
            .thumbnail(SHOWN, SHOWN)
            .to_rgba8()
    } else {
        img
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const THREAD: &str = "=== THREAD PARENT MESSAGE ===\nFrom: Ada <ada@example.com> (U1)\nTime: 2026-11-10 13:30:12 EST\nMessage TS: 1794335412.484889\ncan <@U2|bo> look at <https://linear.app/x/issue/ABC-7/a|the bug>?\nFiles: please see below\nFiles: shot, final.png (ID: F1, image/png, 48.2 KB), clip.mov (ID: F2, video/quicktime, 2.0 MB)\n\n=== THREAD REPLIES (2 total) ===\n\n--- Reply 1 of 2 ---\nFrom: Bo (U2)\nTime: 2026-11-10 13:40:00 EST\nMessage TS: 1794336000.000100\non it\n\n--- Reply 2 of 2 ---\nFrom: Slack relay <bot@example.com> (U9)\nTime: 2026-11-10 13:41:00 EST\nMessage TS: 1794336060.000100\n\nFiles: a.pdf (ID: F3, application/pdf, 1 KB)\n";

    #[test]
    fn a_thread_is_read_with_its_files_and_reply_count() {
        let t = thread_from(THREAD, 7).unwrap();
        assert_eq!(t.replies, 2);
        assert_eq!(t.read, 7);
        assert_eq!(t.messages.len(), 3);
        let first = &t.messages[0];
        assert_eq!((first.user.as_str(), first.from.as_str()), ("U1", "Ada"));
        assert_eq!(
            first.text,
            "can @bo look at the bug?\nFiles: please see below"
        );
        assert_eq!(first.refs, ["ABC-7"], "a link's ticket kept");
        assert_eq!(
            first.files,
            [
                SlackFile {
                    id: "F1".into(),
                    name: "shot, final.png".into(),
                    mimetype: "image/png".into()
                },
                SlackFile {
                    id: "F2".into(),
                    name: "clip.mov".into(),
                    mimetype: "video/quicktime".into()
                }
            ]
        );
        assert_eq!(t.messages[2].from, "Slack relay");
        assert_eq!(t.messages[2].text, "");
        assert_eq!(
            t.files().map(|f| f.id.as_str()).collect::<Vec<_>>(),
            ["F1", "F2"],
            "a PDF isn't drawn"
        );
        assert!(thread_from("nothing here", 0).is_none());
    }

    #[test]
    fn only_pictures_and_videos_whose_bytes_say_so_are_kept() {
        let file = |mimetype: &str| SlackFile {
            id: "F1".into(),
            name: "x.terminal".into(),
            mimetype: mimetype.into(),
        };
        assert_eq!(
            named(&file("video/mp4")).as_deref(),
            Some("F1.mp4"),
            "the name's extension is never used"
        );
        assert_eq!(named(&file("application/x-terminal")), None);
        assert!(!file("video/x-anything").drawn());
        assert!(sniffs_as(b"\x89PNG\r\n\x1a\nrest", "png"));
        assert!(sniffs_as(b"\0\0\0\x18ftypmp42", "mp4"));
        assert!(
            !sniffs_as(b"<?xml version=\"1.0\"?><plist>", "mp4"),
            "a plist claiming to be a video"
        );
    }

    /// Answers the read-thread tool with two pages.
    struct Pages;

    impl Connector for Pages {
        fn call(
            &self,
            _tool: &str,
            input: Value,
            _timeout: Option<std::time::Duration>,
        ) -> impl std::future::Future<Output = Result<String, crate::connector::ConnectorError>> + Send
        {
            let answer = if input.get("cursor").is_none() {
                json!({ "messages": "=== THREAD PARENT MESSAGE ===\nFrom: Ada (U1)\nMessage TS: 1.0\nfirst\n\n=== THREAD REPLIES (2 total) ===\n\n--- Reply 1 of 2 ---\nFrom: Bo (U2)\nMessage TS: 2.0\nsecond\n", "pagination_info": "use cursor `next=`" })
            } else {
                json!({ "messages": "--- Reply 2 of 2 ---\nFrom: Cy (U3)\nMessage TS: 3.0\nthird\n" })
            };
            std::future::ready(Ok(answer.to_string()))
        }
    }

    #[test]
    fn a_thread_is_read_page_by_page() {
        let t = util::block_on(read_thread(&Pages, "C1", "1.0", 0)).unwrap();
        let texts: Vec<&str> = t.messages.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(texts, ["first", "second", "third"]);
        assert_eq!(t.replies, 2);
    }

    #[test]
    fn a_files_bytes_come_from_a_data_uri_or_the_longest_base64() {
        let png = util::base64(b"\x89PNG not really");
        assert_eq!(
            bytes_from(&format!("File ID: F1\ndata:image/png;base64,{png}\n")).unwrap(),
            b"\x89PNG not really"
        );
        let long = vec![7u8; 300];
        let answer = json!({ "content": format!("short AAAA then {}", util::base64(&long)) });
        assert_eq!(bytes_from(&answer.to_string()).unwrap(), long);
        assert!(bytes_from("no data").is_none());
    }

    #[test]
    fn the_skills_threads_are_kept_only_for_messages_still_saved() {
        let state = std::env::temp_dir().join(format!(
            "sb-slack-threads-{}-{}",
            std::process::id(),
            util::now_ms()
        ));
        std::fs::create_dir_all(&state).unwrap();
        let now = util::now_ms();
        let ts = format!("{:.6}", now as f64 / 1000.0 - 60.0);
        std::fs::write(
            slack::file(&state),
            json!({ "checked": now, "items": [{ "kind": "thread", "channel": "C1", "ts": ts, "thread": "1794335412.484889" }] }).to_string(),
        )
        .unwrap();
        let found = [
            json!({ "channel": "C1", "thread": "1794335412.484889", "text": THREAD }),
            json!({ "channel": "C9", "thread": "1.0", "text": json!({ "messages": THREAD }).to_string() }),
            json!({ "channel": "C1;rm", "thread": "1.0", "text": THREAD }),
        ];
        assert_eq!(
            save_found(&state, &found, now).unwrap(),
            2,
            "a bad channel ID is left out"
        );
        let kept = read(&state);
        assert_eq!(
            kept.keys().collect::<Vec<_>>(),
            ["C1:1794335412.484889"],
            "a thread of no saved message isn't kept"
        );
        assert_eq!(kept["C1:1794335412.484889"].replies, 2);
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn a_look_reads_the_threads_with_something_newer_than_what_is_kept() {
        let m = |channel: &str, ts: &str, thread: Option<&str>| slack::Message {
            kind: "thread".into(),
            channel: channel.into(),
            ts: ts.into(),
            thread: thread.map(str::to_string),
            ..Default::default()
        };
        let saved = slack::Saved {
            items: vec![
                m("C1", "10.0", Some("5.0")),
                m("C1", "12.0", Some("5.0")),
                m("C2", "11.0", Some("3.0")),
                m("C3", "13.0", None),
                m("C4", "9.0", Some("1.0")),
            ],
            replied: [
                ("C5:2.0".to_string(), json!("14.0")),
                ("C6:2.0".to_string(), json!("15.0")),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let mut kept = BTreeMap::new();
        kept.insert(
            "C5:2.0".to_string(),
            Thread {
                messages: vec![Said {
                    ts: "2.0".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
        );
        kept.insert(
            "C2:3.0".to_string(),
            Thread {
                messages: vec![Said {
                    ts: "11.0".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
        );
        assert_eq!(
            wanted(&saved, &kept, 8),
            [
                ("C5".to_string(), "2.0".to_string()),
                ("C1".to_string(), "5.0".to_string()),
                ("C4".to_string(), "1.0".to_string())
            ],
            "newest first, a kept thread you wrote in since too; the kept one, the message in no thread and a thread not kept you wrote in left out"
        );
        assert_eq!(wanted(&saved, &kept, 1).len(), 1);
    }
}
