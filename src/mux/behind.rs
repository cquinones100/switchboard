//! Whether Switchboard's main has moved past the build the server runs,
//! said as one notification ("Switchboard's main has 3 landings this build
//! lacks") whose menu leads with M-p's "Restart Switchboard on the latest
//! main" (src/mux/upgrade.rs, src/mux/latest.rs). Without it a landing left
//! the workspace on the build before it until someone thought to restart.
//!
//! The binary knows the commit it was built from (build.rs's
//! `SWITCHBOARD_COMMIT`) and the main clone it belongs to (`latest::checkout`,
//! only a main clone: a release build, or one told to build a checkout as it
//! is, has no main to fall behind; nor does one whose clone isn't
//! Switchboard's, `dev::build_is_from_clone`). The commits on the clone's
//! `origin/<default>` (else its local default branch) that the build lacks
//! are counted with git, off the server's task: as the server starts (so a
//! restart on the latest main counts from there), once an hour on its tick,
//! whenever a tab says it landed straight on main (`land-lock landed`
//! setting `@tab_landed`), and whenever the merge queue merges a PR
//! (src/merge_queue/serve.rs). Each count first fetches the clone's
//! `origin/<default>` while Settings' GitHub is on: a PR the merge queue
//! squash-merges on GitHub, or a push made from another clone, moves
//! nothing in this one, so without the fetch no notice came for them. The
//! notification is one, its count updated as landings come,
//! dismissing it holding until main moves again (the commit it is at, in
//! its data); a server started on a build at or past main finds nothing and
//! shows none. Settings' `build_behind_notice` off, nothing is counted or
//! shown.
//!
//! `SWITCHBOARD_BUILT_FROM` in the server's environment names another
//! commit to count from, for the tests, whose own build's commit isn't in
//! the repository they make.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::items::Item;
use crate::mux::server::{Event, Server};
use crate::settings;

/// The setting that turns the notification off.
pub const SETTING: &str = "build_behind_notice";
/// The notifications' source it is listed under.
pub const SOURCE: &str = "behind";
/// How often the count is made again with nothing landing,
/// `SWITCHBOARD_BEHIND_EVERY` seconds for the tests.
fn every() -> Duration {
    std::env::var("SWITCHBOARD_BEHIND_EVERY")
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s > 0.0)
        .map(Duration::from_secs_f64)
        .unwrap_or(Duration::from_secs(60 * 60))
}

/// What the server keeps: when the count was last asked for, whether one
/// is under way, and the id of the notification put away.
#[derive(Default)]
pub struct Behind {
    asked: Option<Instant>,
    counting: bool,
    /// Asked again while a count was under way: counted again once it
    /// answers, so a landing's count isn't lost behind the hourly one.
    again: bool,
    dismissed: Option<String>,
    /// What the last count found: how many commits, and main's commit.
    found: Option<(usize, String)>,
    /// The setting as the last tick read it, so turning it on counts at
    /// once and turning it off takes the notification away at once.
    on: Option<bool>,
}

/// The commit the running build was made from, when it is known.
pub fn built_from() -> Option<String> {
    let commit = std::env::var("SWITCHBOARD_BUILT_FROM")
        .ok()
        .filter(|c| !c.trim().is_empty())
        .unwrap_or_else(|| env!("SWITCHBOARD_COMMIT").to_string());
    let commit = commit.trim();
    (!commit.is_empty() && commit != "unknown").then(|| commit.to_string())
}

/// Runs git in `clone`, giving what it printed when it succeeded.
fn git(clone: &Path, args: &[&str]) -> Option<String> {
    Command::new("git")
        .arg("-C")
        .arg(clone)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
}

/// The clone's default branch: `origin/HEAD`'s, else `main`.
fn default_branch(clone: &Path) -> String {
    git(
        clone,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .and_then(|r| r.trim().strip_prefix("origin/").map(String::from))
    .unwrap_or_else(|| "main".to_string())
}

/// How long a fetch may take before it is ended, so one stalled on a dead
/// connection never holds the counts after it.
const FETCH_FOR: Duration = Duration::from_secs(60);

/// Brings the clone's `origin/<default>` to where it is on origin, by name
/// whatever origin's refspec fetches; a fetch that fails (offline) or runs
/// past `FETCH_FOR` leaves it as it was, and the count is made from that.
pub fn fetch(clone: &Path) {
    let default = default_branch(clone);
    let refspec = format!("+refs/heads/{default}:refs/remotes/origin/{default}");
    let Ok(mut child) = Command::new("git")
        .arg("-C")
        .arg(clone)
        .args(["fetch", "-q", "--no-tags", "origin", &refspec])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return;
    };
    let started = Instant::now();
    while matches!(child.try_wait(), Ok(None)) {
        if started.elapsed() >= FETCH_FOR {
            let _ = child.kill();
            let _ = child.wait();
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The commits on `clone`'s main that `commit` lacks, and the commit main
/// is at; `None` when git can't say (no such commit, no main).
pub fn count(clone: &Path, commit: &str) -> Option<(usize, String)> {
    let git = |args: &[&str]| git(clone, args);
    let default = default_branch(clone);
    let upstream = [format!("origin/{default}"), default]
        .into_iter()
        .find(|r| git(&["rev-parse", "--verify", "-q", &format!("{r}^{{commit}}")]).is_some())?;
    let listed = git(&[
        "rev-list",
        &format!("{commit}^{{commit}}..{upstream}"),
        "--",
    ])?;
    let commits: Vec<&str> = listed.lines().filter(|l| !l.is_empty()).collect();
    let head = match commits.first() {
        Some(head) => head.to_string(),
        None => git(&["rev-parse", &upstream])?.trim().to_string(),
    };
    Some((commits.len(), head))
}

/// The notification's words for `n` commits.
pub fn words(n: usize) -> String {
    let landings = if n == 1 { "landing" } else { "landings" };
    format!("Switchboard's main has {n} {landings} this build lacks")
}

/// The notification for `n` commits, main at `head`: one id whatever
/// the count, so a landing updates it rather than adding another.
pub fn notice(n: usize, head: &str) -> Item {
    Item::new("main", words(n))
        .detail("Restart Switchboard on the latest main to take them up")
        .data(serde_json::json!({ "head": head }))
}

impl Server {
    /// On the server's tick: the first count, then one an hour, and one
    /// at once whenever the setting changes.
    pub fn behind_tick(&mut self) {
        let on = settings::is_on(SETTING);
        let turned = self.upgrade.behind.on.replace(on) != Some(on);
        if turned
            || self
                .upgrade
                .behind
                .asked
                .is_none_or(|at| at.elapsed() >= every())
        {
            self.behind_check();
        }
    }

    /// Counts again, off the server's task, unless the setting is off or
    /// there is no main clone to count in; a count under way counts again
    /// as it answers.
    pub fn behind_check(&mut self) {
        self.upgrade.behind.asked = Some(Instant::now());
        let clone = super::latest::checkout()
            .filter(|c| c.main_clone && crate::dev::build_is_from_clone())
            .map(|c| c.dir);
        // Nothing to count (the setting off, a release build or one made
        // from a clone that isn't Switchboard's, no commit known) is an answer too: none, so a notification an earlier build
        // gave goes.
        let (Some(clone), Some(commit), true) = (clone, built_from(), settings::is_on(SETTING))
        else {
            self.upgrade.behind.found = None;
            self.sidebar.history.answered.insert(SOURCE.to_string());
            self.behind_show();
            return;
        };
        let behind = &mut self.upgrade.behind;
        if behind.counting {
            behind.again = true;
            return;
        }
        behind.counting = true;
        let fetching = crate::integrations::ensure_on("github", "GitHub").is_ok();
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            if fetching {
                crate::requests::count("GitHub", "git", SOURCE);
                fetch(&clone);
            }
            let found = count(&clone, &commit);
            let _ = tx.send(Event::Upgrade(super::upgrade::UpgradeEvent::Behind(found)));
        });
    }

    /// A count answered.
    pub fn behind_counted(&mut self, found: Option<(usize, String)>) {
        let behind = &mut self.upgrade.behind;
        behind.counting = false;
        // Kept while git can't say, as a lookup that fails keeps its answer.
        // A count is an answer even when it finds nothing, so the
        // notifications' history takes the last one as gone, as after a
        // restart on a build at or past main (none but a list with items
        // counts as one otherwise).
        if found.is_some() {
            behind.found = found;
            self.sidebar.history.answered.insert(SOURCE.to_string());
        }
        let behind = &mut self.upgrade.behind;
        if std::mem::take(&mut behind.again) {
            self.behind_check();
        }
        self.behind_show();
    }

    /// Puts the notification away until main moves again.
    pub fn behind_dismiss(&mut self, item: &Item) {
        let head = item.data.get("head").and_then(serde_json::Value::as_str);
        self.upgrade.behind.dismissed = head.map(String::from);
        self.behind_show();
    }

    /// The notification as the last count found it.
    fn behind_show(&mut self) {
        let behind = &self.upgrade.behind;
        let items = match &behind.found {
            Some((n, head))
                if *n > 0
                    && behind.dismissed.as_deref() != Some(head.as_str())
                    && settings::is_on(SETTING) =>
            {
                vec![notice(*n, head)]
            }
            _ => Vec::new(),
        };
        self.set_notices(SOURCE, items);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn counts_the_commits_on_main_the_build_lacks() {
        let dir = std::env::temp_dir().join(format!("sb-behind-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let origin = dir.join("origin");
        let clone = dir.join("clone");
        git(
            &dir,
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                origin.to_str().unwrap(),
            ],
        );
        git(
            &dir,
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                clone.to_str().unwrap(),
            ],
        );
        git(&clone, &["commit", "-q", "--allow-empty", "-m", "built"]);
        let built = git(&clone, &["rev-parse", "--short=12", "HEAD"]);
        git(&clone, &["push", "-q", "origin", "HEAD:main"]);
        assert_eq!(count(&clone, &built).map(|(n, _)| n), Some(0));
        for n in 1..=3 {
            git(
                &clone,
                &[
                    "commit",
                    "-q",
                    "--allow-empty",
                    "-m",
                    &format!("landed {n}"),
                ],
            );
        }
        // Only what reached origin counts once it has an origin/main.
        assert_eq!(count(&clone, &built).map(|(n, _)| n), Some(0));
        git(&clone, &["push", "-q", "origin", "HEAD:main"]);
        let head = git(&clone, &["rev-parse", "HEAD"]);
        assert_eq!(count(&clone, &built), Some((3, head)));
        // Merged on origin by something else (the merge queue on GitHub, a
        // push from another clone): counted once fetched.
        let other = dir.join("other");
        git(
            &dir,
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                other.to_str().unwrap(),
            ],
        );
        git(&other, &["commit", "-q", "--allow-empty", "-m", "merged"]);
        git(&other, &["push", "-q", "origin", "HEAD:main"]);
        assert_eq!(count(&clone, &built).map(|(n, _)| n), Some(3));
        fetch(&clone);
        let head = git(&other, &["rev-parse", "HEAD"]);
        assert_eq!(count(&clone, &built), Some((4, head)));
        assert_eq!(
            words(3),
            "Switchboard's main has 3 landings this build lacks"
        );
        assert_eq!(
            words(1),
            "Switchboard's main has 1 landing this build lacks"
        );
        assert_eq!(count(&clone, "0123456789ab"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
