//! Which build the phone app's page and service worker came from. After
//! Switchboard restarted on a new build, a phone could keep running the
//! service worker from before until the app was closed and opened again,
//! and nothing on the phone or the computer showed it. So `app.js` and
//! `sw.js` are served with the build's commit in their first constant
//! (`const BUILD = "dev";` in the files, the placeholder replaced as they
//! are served), which also changes `sw.js`'s bytes with each commit, so the
//! phone installs the new worker on its next look. The commit is the
//! binary's (`SWITCHBOARD_COMMIT`), with the clone's `HEAD` after a `+` when
//! the files are read from a clone at another commit (src/assets.rs), read
//! from git's own files rather than by running git.
//!
//! The page sends its own build and its worker's with each request
//! (`X-Switchboard-Build: page=<commit> worker=<commit>`, the worker's
//! `none` while no worker controls the page and `unknown` when one older
//! than this answers nothing), and `Seen` says, for phone.log, when a
//! device that was behind the server's build updated itself to it, or is
//! still behind a minute after it came back, once a device and change.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::watch;

use crate::tabs::git;

/// The request header the page sends its builds in.
pub const HEADER: &str = "x-switchboard-build";

/// The line in `app.js` and `sw.js` that the build replaces.
const PLACEHOLDER: &str = "const BUILD = \"dev\";";

/// The build serving the app: the binary's commit, and the clone's `HEAD`
/// when the files come from `clone` and it is at another commit.
pub fn of(clone: Option<&Path>) -> String {
    stamp(
        env!("SWITCHBOARD_COMMIT"),
        clone.and_then(head_of).as_deref(),
    )
}

fn stamp(binary: &str, head: Option<&str>) -> String {
    let binary = binary.trim();
    match head {
        Some(head) if !head.starts_with(binary) && !binary.starts_with(head) => {
            format!("{binary}+{head}")
        }
        _ => binary.to_string(),
    }
}

/// The commit `clone`'s `HEAD` is at, to `SWITCHBOARD_COMMIT`'s length.
fn head_of(clone: &Path) -> Option<String> {
    let place = git::locate(clone)?;
    let head = std::fs::read_to_string(place.git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    let commit = match head.strip_prefix("ref:") {
        Some(name) => git::ref_at(clone, name.trim())?,
        None => head.to_string(),
    };
    let commit: String = commit.chars().take(12).collect();
    (commit.len() == 12 && commit.chars().all(|c| c.is_ascii_hexdigit())).then_some(commit)
}

/// `text` (`app.js` or `sw.js`) with `build` in its `BUILD` constant.
pub fn stamped(text: &str, build: &str) -> String {
    let build: String = build
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '+')
        .collect();
    text.replacen(PLACEHOLDER, &format!("const BUILD = \"{build}\";"), 1)
}

/// The page's and the worker's builds a request's header names.
fn parse(header: &str) -> (Option<&str>, Option<&str>) {
    let mut page = None;
    let mut worker = None;
    for part in header.split_whitespace() {
        match part.split_once('=') {
            Some(("page", v)) if !v.is_empty() => page = Some(v),
            Some(("worker", v)) if !v.is_empty() => worker = Some(v),
            _ => {}
        }
    }
    (page, worker)
}

/// How often an open stream looks whether the build serving the app
/// changed (a clone's `HEAD` moving, read from git's files), so a page
/// open in front hears of it within a few seconds.
pub const LOOK: Duration = Duration::from_secs(5);

/// The build to send next on a stream that last sent `sent`: once `rx`
/// changes, or once `build_now` reads another; None once `rx` closes.
pub async fn next_on_stream(
    rx: &mut watch::Receiver<Value>,
    sent: &str,
    build_now: impl Fn() -> String + Clone + Send + 'static,
) -> Option<String> {
    let read = || async {
        tokio::task::spawn_blocking(build_now.clone())
            .await
            .unwrap_or_default()
    };
    loop {
        tokio::select! {
            changed = rx.changed() => {
                changed.ok()?;
                return Some(read().await);
            }
            () = tokio::time::sleep(LOOK) => {
                let build = read().await;
                if build != sent {
                    return Some(build);
                }
            }
        }
    }
}

/// `tabs`, the stream's message, with the build serving the app in it.
pub fn with_build(mut tabs: Value, build: &str) -> String {
    if let Some(fields) = tabs.as_object_mut() {
        fields.insert("build".into(), Value::from(build));
    }
    tabs.to_string()
}

/// How long a device may stay behind the build before phone.log says so:
/// the page updates itself as it comes back (`assets/phone/app.js`'s
/// `catchUp`), which takes a moment.
pub const LATE: Duration = Duration::from_secs(60);

/// A device found behind the build: the old builds it ran (`key`), since
/// when, the line phone.log would say, and whether it said it.
struct Behind {
    key: String,
    since: i64,
    line: String,
    said: bool,
}

/// What a request's builds said of its device.
#[derive(Debug, PartialEq)]
pub enum Noted {
    /// Nothing new.
    Quiet,
    /// It fell behind the build: `overdue` says, after `LATE`, whether it
    /// still is.
    FellBehind,
    /// It was behind, and now runs the server's build: the line for
    /// phone.log.
    Updated(String),
}

/// Which devices are behind the build, and since when, so phone.log says
/// one updated itself, or is still behind a minute on, once a change
/// rather than on every request.
#[derive(Default)]
pub struct Seen(Mutex<HashMap<String, Behind>>);

impl Seen {
    /// What `device`'s page and worker, as `header` names them, say against
    /// `server` at `now` (ms): fallen behind, or caught up after being
    /// behind, which forgets it, so falling behind again is said again.
    pub fn note(&self, device: &str, header: &str, server: &str, now: i64) -> Noted {
        let (page, worker) = parse(header);
        // A page names its worker's build only once the worker answers, a
        // moment after it starts: until then nothing is known of the worker,
        // so nothing is said or forgotten.
        let (Some(page), Some(worker)) = (page, worker) else {
            return Noted::Quiet;
        };
        let behind = page != server || (worker != "none" && worker != server);
        let mut seen = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if !behind {
            return match seen.remove(device) {
                Some(_) => Noted::Updated(format!("{device} updated itself to {server}")),
                None => Noted::Quiet,
            };
        }
        // Keyed on the old builds the device still runs, whichever side runs
        // them: updating brings the worker up first and the page a moment
        // later (reopening, the other way round), which is the same change
        // catching up.
        let mut old: Vec<&str> = [page, worker]
            .into_iter()
            .filter(|b| *b != "none" && *b != server)
            .collect();
        old.dedup();
        let key = format!("{} {server}", old.join(" "));
        let line = format!(
            "{device} is behind the build {server} a minute after it came back: its page is {page}, its service worker {worker}; closing and reopening the app takes up the new one"
        );
        if let Some(known) = seen.get_mut(device).filter(|b| b.key == key) {
            known.line = line;
            return Noted::Quiet;
        }
        seen.insert(
            device.to_string(),
            Behind {
                key,
                since: now,
                line,
                said: false,
            },
        );
        Noted::FellBehind
    }

    /// The line for phone.log when `device` is still behind `LATE` after it
    /// fell behind, at `now` (ms), and it wasn't said.
    pub fn overdue(&self, device: &str, now: i64) -> Option<String> {
        let mut seen = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let behind = seen.get_mut(device)?;
        let is_late = now - behind.since >= LATE.as_millis() as i64;
        if behind.said || !is_late {
            return None;
        }
        behind.said = true;
        Some(behind.line.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stamp_is_the_binarys_commit_and_the_clones_when_it_differs() {
        assert_eq!(stamp("1a2b3c4d5e6f", None), "1a2b3c4d5e6f");
        assert_eq!(stamp("1a2b3c4d5e6f", Some("1a2b3c4d5e6f")), "1a2b3c4d5e6f");
        assert_eq!(
            stamp("1a2b3c4d5e6f", Some("9f8e7d6c5b4a")),
            "1a2b3c4d5e6f+9f8e7d6c5b4a"
        );
    }

    #[test]
    fn the_app_and_its_worker_carry_the_placeholder_the_build_replaces() {
        for text in [
            include_str!("../../assets/phone/app.js"),
            include_str!("../../assets/phone/sw.js"),
        ] {
            assert!(text.contains(PLACEHOLDER));
            let served = stamped(text, "1a2b3c4d5e6f");
            assert!(served.contains("const BUILD = \"1a2b3c4d5e6f\";"));
            assert!(!served.contains(PLACEHOLDER));
        }
    }

    #[test]
    fn the_worker_served_by_two_commits_differs_in_its_bytes() {
        let sw = include_str!("../../assets/phone/sw.js");
        assert_ne!(stamped(sw, "1a2b3c4d5e6f"), stamped(sw, "9f8e7d6c5b4a"));
    }

    #[test]
    fn a_stamp_cannot_end_the_string_it_sits_in() {
        assert_eq!(
            stamped(PLACEHOLDER, "ab\"; alert(1); \"cd"),
            "const BUILD = \"abalert1cd\";"
        );
    }

    #[test]
    fn a_device_behind_is_noted_once_per_change_and_said_only_when_late() {
        let seen = Seen::default();
        let late = LATE.as_millis() as i64;
        assert_eq!(
            seen.note("phone", "page=new worker=old", "new", 0),
            Noted::FellBehind
        );
        assert_eq!(
            seen.note("phone", "page=new worker=old", "new", 1),
            Noted::Quiet
        );
        // Each load's first request, before the worker answers, changes
        // nothing.
        assert_eq!(seen.note("phone", "page=new", "new", 2), Noted::Quiet);
        assert_eq!(seen.overdue("phone", late - 1), None);
        let line = seen.overdue("phone", late).expect("still behind");
        assert!(
            line.contains("phone") && line.contains("old") && line.contains("new"),
            "{line}"
        );
        assert_eq!(seen.overdue("phone", late + 1), None);
        assert_eq!(
            seen.note("tablet", "page=new worker=old", "new", 0),
            Noted::FellBehind
        );
        // Resumed across a restart with both old, then updating itself: the
        // worker is new before the page is, which is the same change.
        assert_eq!(
            seen.note("ipad", "page=old worker=old", "new", 0),
            Noted::FellBehind
        );
        assert_eq!(
            seen.note("ipad", "page=old worker=new", "new", 1),
            Noted::Quiet
        );
        assert_eq!(
            seen.note("ipad", "page=new worker=new", "new", 2),
            Noted::Updated("ipad updated itself to new".into())
        );
        // Updated in time, it is never said to be behind.
        assert_eq!(seen.overdue("ipad", late), None);
        // Caught up, then behind again: noted again.
        assert!(matches!(
            seen.note("phone", "page=new worker=new", "new", 3),
            Noted::Updated(_)
        ));
        assert_eq!(
            seen.note("phone", "page=new worker=new", "new", 4),
            Noted::Quiet
        );
        assert_eq!(
            seen.note("phone", "page=new worker=old", "new", 5),
            Noted::FellBehind
        );
        // Another build on the server is another change.
        assert_eq!(
            seen.note("phone", "page=new worker=old", "newer", 6),
            Noted::FellBehind
        );
    }

    #[test]
    fn a_page_with_no_worker_or_no_header_is_never_behind_for_it() {
        let seen = Seen::default();
        assert_eq!(
            seen.note("phone", "page=new worker=none", "new", 0),
            Noted::Quiet
        );
        assert_eq!(seen.note("phone", "page=old", "new", 0), Noted::Quiet);
        assert_eq!(seen.note("phone", "", "new", 0), Noted::Quiet);
        assert_eq!(
            seen.note("phone", "page=old worker=none", "new", 0),
            Noted::FellBehind
        );
        assert_eq!(seen.note("phone", "page=old", "new", 0), Noted::Quiet);
        assert_eq!(
            seen.note("phone", "page=old worker=none", "new", 0),
            Noted::Quiet
        );
    }
}
