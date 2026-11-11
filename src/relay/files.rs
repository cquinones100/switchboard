//! Images and videos, both ways (lib/slack-relay/files.js).
//!
//! An image attached to a message you send is downloaded, readable only by
//! you, and its path is added to what is typed into the tab: Claude Code
//! reads an image file it is pointed at, so Claude sees the screenshot.
//! Claude can't watch a video, so a video is downloaded the same way and
//! `FRAMES` stills spread evenly through it are taken with ffmpeg, and
//! their paths are typed after the video's, in order. Only images up to
//! `MAX_FILE_BYTES` and videos up to `MAX_VIDEO_BYTES`, and only from
//! Slack's own file host, with the bot's token. Downloads older than
//! `KEEP_FILES_MS` are deleted, stills included.
//!
//! When a turn is posted, the image and video files it made go under it: a
//! screenshot or recording Playwright took, a chart Claude saved. They are
//! the ones Claude's own messages in the turn name, as paths ending in an
//! image or video extension: a file only a tool call or its output names
//! was made along the way, not for you. They are only shared when the file
//! really is one (by its first bytes), was written during the turn, is
//! under the size limit, and isn't one you sent. A path with spaces in it
//! is found when it is written whole between backticks or double quotes.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::Result;
use regex::Regex;
use serde_json::{Value, json};

use super::Relay;
use super::post::{Upload, upload_files};
use super::slack::FileRef;
use super::slices::sliced;
use super::tabs;
use crate::util;

pub const IMAGE_TYPES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];
pub const VIDEO_TYPES: &[&str] = &["video/mp4", "video/quicktime", "video/webm"];
pub const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;
pub const MAX_VIDEO_BYTES: u64 = 200 * 1024 * 1024;
pub const FRAMES: usize = 8;
/// Wide enough to read text in a screen recording; a larger video is
/// scaled down.
const FRAME_WIDTH: u32 = 1280;
pub const KEEP_FILES_MS: i64 = 7 * 24 * 60 * 60_000;
pub const FILE_HOST: &str = "files.slack.com";
pub const MAX_SHARED: usize = 20;

/// A file's own name, cut down to characters that are safe in a path typed
/// into a shell-like prompt.
pub fn safe_name(name: &str) -> String {
    static UNSAFE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\w.-]+").expect("a regex"));
    let cleaned = UNSAFE.replace_all(name, "-");
    let cleaned = cleaned.trim_start_matches(['.', '-']);
    let cut: String = cleaned.chars().take(80).collect();
    if cut.is_empty() { "image".into() } else { cut }
}

/// What came of a file you attached.
#[derive(Debug, PartialEq)]
pub enum Got {
    /// It isn't passed on, and why.
    Refused(String),
    Image(PathBuf),
    Video {
        path: PathBuf,
        frames: Vec<PathBuf>,
    },
}

/// The host of an https link, without its port (as `URL.hostname` gives
/// it), or `None` for another kind of link.
fn https_host(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("https://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let host = &rest[..end];
    if host.contains('@') {
        return None;
    }
    Some(match host.rsplit_once(':') {
        Some((name, port)) if port.bytes().all(|b| b.is_ascii_digit()) => name,
        _ => host,
    })
}

/// Downloads one attached file. An event may carry only the file's ID, so
/// the rest is asked of Slack.
pub async fn download(cx: &Relay, file: &FileRef) -> Result<Got> {
    let info = if file.url.is_some() {
        file.clone()
    } else {
        let got = cx
            .slack
            .api("files.info", json!({ "file": file.id }))
            .await?;
        FileRef::from_json(got.get("file").unwrap_or(&Value::Null))
    };
    let mime = info.mimetype.as_deref().unwrap_or("");
    let video = VIDEO_TYPES.contains(&mime);
    if !video && !IMAGE_TYPES.contains(&mime) {
        return Ok(Got::Refused(format!(
            "{} isn't an image or video I can pass on (PNG, JPEG, GIF, WebP, MP4, MOV or WebM).",
            info.name.as_deref().unwrap_or("That file")
        )));
    }
    let kind = if video { "video" } else { "image" };
    let limit = if video {
        MAX_VIDEO_BYTES
    } else {
        MAX_FILE_BYTES
    };
    let called = |that: &str| {
        info.name
            .clone()
            .unwrap_or_else(|| format!("{that} {kind}"))
    };
    let too_big = || {
        Got::Refused(format!(
            "{} is larger than {} MB.",
            called("That"),
            limit / 1024 / 1024
        ))
    };
    if info.size.is_some_and(|s| s > limit) {
        return Ok(too_big());
    }
    let Some(url) = info.url.clone() else {
        return Ok(Got::Refused(format!(
            "Slack gave no address for {}.",
            called("that")
        )));
    };
    if https_host(&url) != Some(FILE_HOST) {
        return Ok(Got::Refused(format!(
            "{} isn't on Slack's file host, so I didn't fetch it.",
            called("That")
        )));
    }
    let bytes = cx.slack.get(&url).await?;
    if bytes.len() as u64 > limit {
        return Ok(too_big());
    }
    let dir = cx.paths.files_dir();
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)?;
    }
    let target = dir.join(format!(
        "{}-{}",
        safe_name(&info.id),
        safe_name(info.name.as_deref().unwrap_or("image"))
    ));
    util::write_private(&target, &bytes, false)?;
    cx.log(&format!(
        "{kind}: saved {} ({} bytes)",
        info.name.as_deref().unwrap_or(""),
        bytes.len()
    ));
    Ok(if video {
        let frames = frames_of(cx, &target).await;
        Got::Video {
            path: target,
            frames,
        }
    } else {
        Got::Image(target)
    })
}

/// `FRAMES` stills from a video, evenly spaced from its start, as JPEG
/// files beside it, or none if ffmpeg can't read it (or isn't installed).
pub async fn frames_of(cx: &Relay, video: &Path) -> Vec<PathBuf> {
    let name = video
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let result: Result<Vec<PathBuf>> = async {
        let probe = util::run_async(
            tokio::process::Command::new("ffprobe")
                .args([
                    "-v",
                    "error",
                    "-show_entries",
                    "format=duration",
                    "-of",
                    "csv=p=0",
                ])
                .arg(video),
            Some(Duration::from_secs(30)),
        )
        .await?;
        let duration: f64 = String::from_utf8_lossy(&probe.stdout)
            .trim()
            .parse()
            .unwrap_or(0.0);
        if duration <= 0.0 {
            return Ok(Vec::new());
        }
        let pattern = format!("{}-frame-%02d.jpg", video.display());
        util::run_async(
            tokio::process::Command::new("ffmpeg")
                .args(["-v", "error", "-y", "-i"])
                .arg(video)
                .args([
                    "-vf",
                    &format!(
                        "fps={:.6},scale='min({FRAME_WIDTH},iw)':-2",
                        FRAMES as f64 / duration
                    ),
                    "-frames:v",
                    &FRAMES.to_string(),
                    &pattern,
                ]),
            Some(Duration::from_secs(120)),
        )
        .await?;
        Ok((1..=FRAMES)
            .map(|i| PathBuf::from(pattern.replace("%02d", &format!("{i:02}"))))
            .filter(|f| f.exists())
            .collect())
    }
    .await;
    result.unwrap_or_else(|e| {
        cx.log(&format!("stills from {name} failed: {e}"));
        Vec::new()
    })
}

/// Deletes downloads older than `KEEP_FILES_MS`.
pub fn prune_files(cx: &Relay) {
    let Ok(entries) = std::fs::read_dir(cx.paths.files_dir()) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age.as_millis() as i64 > KEEP_FILES_MS);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Whether a file's first bytes are an image's.
pub fn looks_like_image(head: &[u8]) -> bool {
    head.len() >= 12
        && ((head[0] == 0x89 && &head[1..4] == b"PNG")
            || (head[0] == 0xff && head[1] == 0xd8 && head[2] == 0xff)
            || &head[..4] == b"GIF8"
            || (&head[..4] == b"RIFF" && &head[8..12] == b"WEBP"))
}

/// MP4 and MOV start with a size and "ftyp"; WebM with Matroska's mark.
pub fn looks_like_video(head: &[u8]) -> bool {
    head.len() >= 8 && (&head[4..8] == b"ftyp" || head[..4] == [0x1a, 0x45, 0xdf, 0xa3])
}

/// The image and video paths a text names, in the order it names them. A
/// span between backticks or double quotes that may be one path with
/// spaces is listed whole, before the pieces the pattern for a path
/// without spaces finds in it, since only the file system can tell
/// `./render.sh /tmp/out.png` from `./my shots/out.png`: `turn_media`
/// shares only what is a real file.
pub fn media_paths(text: &str) -> Vec<String> {
    let mut found: Vec<(usize, String)> = quoted_paths(text);
    found.extend(unquoted_paths(text));
    found.sort_by_key(|(start, _)| *start);
    let mut paths: Vec<String> = Vec::new();
    for (_, path) in found {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
}

/// The spans between backticks or double quotes, on one line, that hold a
/// space, start as a path does (`/`, `~/`, `./`, `../`, or a first word
/// holding a `/`) and end in an image or video extension, with where each
/// starts.
fn quoted_paths(text: &str) -> Vec<(usize, String)> {
    // Each kind of quote on its own, so a path in double quotes inside a
    // command in backticks is found too.
    static QUOTED: LazyLock<[Regex; 2]> = LazyLock::new(|| {
        [r"`([^`\n]+)`", r#""([^"\n]+)""#].map(|r| Regex::new(r).expect("a regex"))
    });
    static MEDIA: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\.(?:png|jpe?g|gif|webp|mp4|mov|webm)$").expect("a regex")
    });
    let mut spans = Vec::new();
    for c in QUOTED.iter().flat_map(|q| q.captures_iter(text)) {
        let m = c.get(1).expect("the span");
        let span = m.as_str();
        let first_word = span.split(' ').next().unwrap_or("");
        let starts_as_a_path = ["/", "~/", "./", "../"].iter().any(|p| span.starts_with(p))
            || first_word.contains('/');
        if span.contains(' ') && starts_as_a_path && MEDIA.is_match(span) {
            spans.push((m.start(), span.to_string()));
        }
    }
    spans
}

/// The paths without spaces a text names, with where each starts.
fn unquoted_paths(text: &str) -> Vec<(usize, String)> {
    static PATH: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?i)(?:^|[\s"'`(=:\[])((?:~|\.{1,2})?/?[\w@%+.,/-]*\.(?:png|jpe?g|gif|webp|mp4|mov|webm))"#)
            .expect("a regex")
    });
    let mut found = Vec::new();
    for c in PATH.captures_iter(text) {
        let m = c.get(1).expect("the path");
        // What follows must end the path: the end, a space or quote, a
        // closing bracket, punctuation, or a full stop ending a sentence.
        let after = &text[m.end()..];
        let mut next = after.chars();
        let ends = match next.next() {
            None => true,
            Some(c) if c.is_whitespace() || "\"'`)],:;!?".contains(c) => true,
            Some('.') => next.next().is_none_or(char::is_whitespace),
            _ => false,
        };
        if ends {
            found.push((m.start(), m.as_str().to_string()));
        }
    }
    found
}

/// The images and videos a transcript's last turn made, as the module's
/// comment says.
pub fn turn_media(cx: &Relay, transcript: &Path) -> Vec<PathBuf> {
    let Some(turn) = tabs::last_prompt(transcript) else {
        return Vec::new();
    };
    let Some(sent) = turn.sent_at else {
        return Vec::new();
    };
    // Each of Claude's texts, with the directory the session was in when it
    // said it, so a relative path resolves the way Claude meant it.
    let mut cwd = cx.cwd.clone();
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in &turn.after {
        if let Some(dir) = entry.get("cwd").and_then(Value::as_str) {
            cwd = PathBuf::from(dir);
        }
        if entry.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        for part in entry
            .pointer("/message/content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|p| p.get("type").and_then(Value::as_str) == Some("text"))
        {
            let text = part.get("text").and_then(Value::as_str).unwrap_or("");
            for p in media_paths(text) {
                let p = match p.strip_prefix("~/") {
                    Some(rest) => cx.paths.home.join(rest),
                    None => PathBuf::from(&p),
                };
                let full = normalize(&cwd.join(p));
                if !paths.contains(&full) {
                    paths.push(full);
                }
            }
        }
    }
    let files_dir = cx.paths.files_dir();
    paths
        .into_iter()
        .filter(|file| {
            if file.starts_with(&files_dir) {
                return false;
            }
            let Ok(meta) = std::fs::metadata(file) else {
                return false;
            };
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            if !meta.is_file() || modified < sent - 2_000 {
                return false;
            }
            let mut head = [0u8; 12];
            let read = std::fs::File::open(file)
                .and_then(|mut f| std::io::Read::read(&mut f, &mut head))
                .unwrap_or(0);
            let head = &head[..read];
            if looks_like_image(head) {
                meta.len() <= MAX_FILE_BYTES
            } else {
                looks_like_video(head) && meta.len() <= MAX_VIDEO_BYTES
            }
        })
        .take(MAX_SHARED)
        .collect()
}

/// A path with its `.` and `..` worked out, as `path.resolve` does.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Uploads the turn's images and videos into `thread`, together, a tall
/// image as its slices. A failure is only logged: the turn itself is
/// already posted.
pub async fn share_media(cx: &Relay, transcript: Option<&Path>, thread: &str) {
    let Some(transcript) = transcript else {
        return;
    };
    let mut uploads = Vec::new();
    for file in turn_media(cx, transcript) {
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match std::fs::read(&file) {
            Ok(bytes) => uploads.extend(sliced(&name, bytes).into_iter().map(
                |(filename, content)| Upload {
                    title: filename.clone(),
                    filename,
                    content,
                },
            )),
            Err(e) => cx.log(&format!("sharing {} failed: {e}", file.display())),
        }
    }
    if uploads.is_empty() {
        return;
    }
    let failed = |name: &str, e: &anyhow::Error| cx.log(&format!("sharing {name} failed: {e}"));
    match upload_files(cx, uploads, Some(thread), &failed).await {
        Ok(names) if !names.is_empty() => cx.log(&format!("shared {}", names.join(", "))),
        Ok(_) => {}
        Err(e) => cx.log(&format!("sharing failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_claude_names_are_found_and_nothing_that_only_looks_like_one() {
        assert_eq!(
            media_paths("Saved `shots/login.png` and /tmp/a.mp4. Also (x.webp), see b.PNG!"),
            ["shots/login.png", "/tmp/a.mp4", "x.webp", "b.PNG"]
        );
        assert!(media_paths("see file.png.bak").is_empty());
    }

    #[test]
    fn a_path_with_spaces_is_found_whole_in_backticks_or_quotes() {
        assert_eq!(
            media_paths("Saved `/tmp/a b/Board 50 - Home.png` and \"~/Shots/My shot (1).jpg\"."),
            [
                "/tmp/a b/Board 50 - Home.png",
                "Home.png",
                "~/Shots/My shot (1).jpg"
            ]
        );
        assert_eq!(
            media_paths("Ran `./render.sh /tmp/out.png`, then `cp shots/a.png b.png`."),
            [
                "./render.sh /tmp/out.png",
                "/tmp/out.png",
                "shots/a.png",
                "b.png"
            ],
            "a command's files are still found, the file system telling which is real"
        );
        assert_eq!(
            media_paths("Ran `open \"/tmp/my shots/out.png\"`."),
            ["/tmp/my shots/out.png", "shots/out.png"],
            "a path in double quotes inside backticks"
        );
        assert!(media_paths("`/tmp/a b/Board 50`").is_empty());
        assert_eq!(
            media_paths("`/tmp/a b/Board\n50 - Home.png`"),
            ["Home.png"],
            "a span ends at a newline"
        );
        assert!(
            media_paths("`my shot.png` is \"a plain sentence.png\"")
                .iter()
                .all(|p| !p.contains(' ')),
            "a span that doesn't start as a path is prose"
        );
    }

    #[test]
    fn names_are_made_safe_for_a_path() {
        assert_eq!(safe_name("my screen shot (1).png"), "my-screen-shot-1-.png");
        assert_eq!(safe_name("../../etc"), "etc");
        assert_eq!(safe_name(""), "image");
    }

    #[test]
    fn only_slacks_file_host_over_https() {
        assert_eq!(
            https_host("https://files.slack.com/f/x.png"),
            Some(FILE_HOST)
        );
        assert_eq!(
            https_host("https://files.slack.com:443/f/x.png"),
            Some(FILE_HOST),
            "a port is no part of the host"
        );
        assert_eq!(https_host("http://files.slack.com/x"), None);
        assert_eq!(https_host("https://evil@files.slack.com/x"), None);
        assert_eq!(
            https_host("https://files.slack.com.evil.com/x"),
            Some("files.slack.com.evil.com")
        );
    }
}
