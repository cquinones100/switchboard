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
//! than this answers nothing), and `Seen` says when a device's page or
//! worker is behind the server's build, once a device and change, for
//! phone.log.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

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

/// What was last said of each device, so a device behind is logged once a
/// change rather than on every request.
#[derive(Default)]
pub struct Seen(Mutex<HashMap<String, String>>);

impl Seen {
    /// The line for phone.log when `device`'s page or worker, as `header`
    /// names them, is behind `server` and wasn't already said; a device
    /// caught up is forgotten, so falling behind again is said again.
    pub fn note(&self, device: &str, header: &str, server: &str) -> Option<String> {
        let (page, worker) = parse(header);
        let page = page?;
        let worker = worker.unwrap_or("none");
        let behind = page != server || (worker != "none" && worker != server);
        let mut seen = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if !behind {
            seen.remove(device);
            return None;
        }
        let key = format!("{page} {worker} {server}");
        if seen.get(device) == Some(&key) {
            return None;
        }
        seen.insert(device.to_string(), key);
        Some(format!(
            "{device} is behind the build {server}: its page is {page}, its service worker {worker}; closing and reopening the app takes up the new one"
        ))
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
    fn a_worker_behind_is_logged_once_per_change() {
        let seen = Seen::default();
        let line = seen
            .note("phone", "page=new worker=old", "new")
            .expect("the worker is behind");
        assert!(
            line.contains("phone") && line.contains("old") && line.contains("new"),
            "{line}"
        );
        assert_eq!(seen.note("phone", "page=new worker=old", "new"), None);
        assert!(seen.note("tablet", "page=new worker=old", "new").is_some());
        // Caught up, then behind again: said again.
        assert_eq!(seen.note("phone", "page=new worker=new", "new"), None);
        assert!(seen.note("phone", "page=new worker=old", "new").is_some());
        // Another build on the server is another change.
        assert!(seen.note("phone", "page=new worker=old", "newer").is_some());
    }

    #[test]
    fn a_page_with_no_worker_or_no_header_is_never_behind_for_it() {
        let seen = Seen::default();
        assert_eq!(seen.note("phone", "page=new worker=none", "new"), None);
        assert_eq!(seen.note("phone", "page=new", "new"), None);
        assert_eq!(seen.note("phone", "", "new"), None);
        assert!(seen.note("phone", "page=old worker=none", "new").is_some());
    }
}
