//! Your settings for each repository, in `~/.config/switchboard/repos.json`,
//! by its origin's `owner/name` on GitHub:
//!
//! ```json
//! { "owner/name": { "lands": "direct", "test": "npm test", "squash": true,
//!                   "testIn": "here", "review": "review-loop", "beforePush": "",
//!                   "afterRebase": "cargo clippy --all-targets", "tickets": "github",
//!                   "issueLabel": "autopilot",
//!                   "apply": [{ "paths": ["db/**"], "label": "Reload the dev server", "command": "make reload" }] } }
//! ```
//!
//! `lands` is the one way its work lands (`Way`): `switchboard-queue`
//! (Switchboard's merge queue), `github-queue` (GitHub's merge queue),
//! `auto-merge` (GitHub's auto-merge), `direct` (straight on its default
//! branch) or `service` (a merge service, handed a PR by the label
//! `serviceLabel` names, the comment `serviceComment` names or both, and
//! watched by the check `serviceCheck` names). Nothing guesses it: a repository
//! with no `lands` lands nothing, `switchboard landing` failing with the
//! words that name Settings' row for it (`not_set_words`), and
//! queue-and-clean and autopilot stopping there. Settings' Repositories
//! checks that each way is set up, from what GitHub said when last asked
//! (`Setup`, kept in the state directory's `repos-setup.json`, a merge
//! service's by its label among the repository's and its check on its
//! recent PRs, `Names`), and may
//! suggest a way from GitHub's rules, which only you choose. A branch keeps
//! the way it started landing with (`branch.<branch>.wtnLands`, written by
//! `switchboard landing --start`), so a change of way applies from the next
//! landing and a branch landing already finishes the old way.
//!
//! The rest of the landing keys are how a direct landing goes: `test` the
//! command run before the review
//! and again after the last rebase (empty, none; not set, what the
//! repository's CLAUDE.md names), `testIn` where they run, `here` (in the
//! worktree) or `ci` (the repository's CI, on a branch of the landing's own
//! pushed for it), `squash` whether the branch lands as one commit, `review` `review-loop` or `none`, `beforePush` a command
//! that must pass before the push (empty, none), and `afterRebase` a command
//! run after the last rebase in place of the tests, when that rebase had no
//! conflict and the default branch's CI isn't failing (empty or not set, the
//! tests run again), since a clean rebase changes nothing the tests haven't
//! seen pass on each side. `tickets` is where the repository's work is
//! kept, `github` (its issues), `linear` or `notes` (a note's checklist
//! and its plans), the one place a repository's tickets are decided: `wt
//! <number>` starts its work from it (`tickets_at`), M-i opens a tab's
//! ticket from it (`tickets_known`), `/shape` files new work in it
//! (`switchboard tickets`) and autopilot takes its work from it. It has no
//! default: while it isn't set, each of them refuses, naming how to set it
//! (`unset_words`). `issueLabel`, for `github`, is the label an issue
//! carries for autopilot to take it, with no default either, so autopilot
//! takes no issues while it isn't set. `apply` is what a change
//! landed there needs to take effect, which `switchboard apply-actions`
//! lists in place of what it would work out by itself. The file is yours
//! and never in this repository; Settings changes it (`set_repo`), never
//! over a file it can't read.
//!
//! Before `lands`, a repository's `landing` (`pr` or `direct`) was guessed
//! when unset, and Settings' `pr_merge_queue` chose between Switchboard's
//! merge queue and GitHub's auto-merge for all of them; a new build's
//! server carries those over once (`migrate_lands`), and a landing that
//! finds an old `landing` with no `lands` carries it over then
//! (`carry_over_in`). Ported from
//! lib/repos.js and bin/apply-actions' `entry`.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::integrations::github;
use crate::{paths, settings, util, worktree};

/// `~/.config/switchboard/repos.json`: always this computer's own, never
/// read from a repository your files are kept in (src/files), since a
/// landing runs the commands it holds.
pub fn file() -> PathBuf {
    paths::local_config_dir().join("repos.json")
}

/// `owner/name` of a git remote's URL on GitHub, as the URL has them, or
/// `None` for one elsewhere.
pub fn repo_of_url(url: &str) -> Option<String> {
    github::parse_remote(url).map(|r| format!("{}/{}", r.owner, r.name))
}

/// `owner/name` of the origin of the repository at `dir`, or `None` outside
/// one or with an origin not on GitHub. Runs git, so never on the drawing
/// path.
pub fn repo_at(dir: &Path) -> Option<String> {
    let url = worktree::git_at(dir, &["remote", "get-url", "origin"]).ok()?;
    repo_of_url(&url)
}

/// The file as it is: empty when it isn't there; an error, saying why, when
/// it can't be read as a JSON object, so nothing writes over it.
pub fn read() -> Result<Map<String, Value>, String> {
    read_file(&file())
}

/// The file, or nothing when it can't be read.
pub fn read_safely() -> Map<String, Value> {
    read().unwrap_or_default()
}

/// A repository's entry, by its `owner/name`, which GitHub takes in any
/// case, so the file's may be written in another; empty when there is none.
pub fn entry_of(data: &Map<String, Value>, repo: &str) -> Map<String, Value> {
    let found = data
        .get(repo)
        .or_else(|| {
            data.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(repo))
                .map(|(_, v)| v)
        })
        .and_then(Value::as_object);
    found.cloned().unwrap_or_default()
}

/// One action of an entry's `apply` list.
#[derive(Clone, Debug, PartialEq)]
pub struct Apply {
    /// The files that call for it, as globs: `*` within a folder, `**` any
    /// number of them, `?` one character.
    pub paths: Vec<String>,
    pub label: String,
    /// A shell command run in the clone; empty for one only you can do.
    pub command: String,
}

/// The `apply` list of the entry for the repository at `dir`, or `None`
/// when it has none (or isn't on GitHub), and then the defaults decide.
pub fn apply_of(dir: &Path) -> Option<Vec<Apply>> {
    let repo = repo_at(dir)?;
    let entry = entry_of(&read_safely(), &repo);
    let list = entry.get("apply")?.as_array()?;
    Some(
        list.iter()
            .filter_map(Value::as_object)
            .map(|a| Apply {
                paths: a
                    .get("paths")
                    .and_then(Value::as_array)
                    .map(|p| {
                        p.iter()
                            .filter_map(Value::as_str)
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default(),
                label: a.get("label").map(util::js_string).unwrap_or_default(),
                command: a
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            })
            .collect(),
    )
}

// ── How a repository lands ──────────────────────────────────────────────────

/// The one way a repository's work lands, its `lands`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Way {
    /// Switchboard's merge queue (src/merge_queue): trains of PRs tested
    /// together on a `ci/train-*` branch and squash-merged in order.
    SwitchboardQueue,
    /// GitHub's merge queue on the default branch.
    GithubQueue,
    /// GitHub's auto-merge, each PR merged once its checks pass.
    AutoMerge,
    /// Straight on the default branch, with no PR.
    Direct,
    /// A merge service, which merges a PR handed to it by its label or
    /// its comment.
    Service,
}

/// One fact of a way, as the menu of ways shows it under the chosen one.
pub type Fact = (&'static str, &'static str);

impl Way {
    /// Every way, in the order the menu lists them.
    pub const ALL: [Way; 5] = [
        Way::SwitchboardQueue,
        Way::GithubQueue,
        Way::AutoMerge,
        Way::Direct,
        Way::Service,
    ];

    /// Its name in repos.json and `switchboard landing`.
    pub fn key(self) -> &'static str {
        match self {
            Way::SwitchboardQueue => "switchboard-queue",
            Way::GithubQueue => "github-queue",
            Way::AutoMerge => "auto-merge",
            Way::Direct => "direct",
            Way::Service => "service",
        }
    }

    pub fn parse(key: &str) -> Option<Way> {
        Way::ALL.into_iter().find(|w| w.key() == key)
    }

    /// What Settings calls it.
    pub fn label(self) -> &'static str {
        match self {
            Way::SwitchboardQueue => "Switchboard's merge queue",
            Way::GithubQueue => "GitHub's merge queue",
            Way::AutoMerge => "GitHub's auto-merge",
            Way::Direct => "Straight to the default branch",
            Way::Service => "A merge service",
        }
    }

    /// A few words beside it in the menu (board 192).
    pub fn aside(self) -> &'static str {
        match self {
            Way::SwitchboardQueue => "PRs, trains tested together",
            Way::GithubQueue => "PRs, GitHub queues them",
            Way::AutoMerge => "PRs, each brought up to date",
            Way::Direct => "no PR: the lock, tests, a push",
            Way::Service => "PRs handed to a service",
        }
    }

    /// Whether its work lands through a pull request.
    pub fn through_pr(self) -> bool {
        self != Way::Direct
    }

    /// Its name on the kit's chip, in the sidebar's Landing group and on a
    /// tab's card (boards 193 to 196).
    pub fn chip(self) -> &'static str {
        match self {
            Way::SwitchboardQueue => "Switchboard's queue",
            Way::GithubQueue => "GitHub's queue",
            Way::AutoMerge => "auto-merge",
            Way::Direct => "straight on",
            Way::Service => "merge service",
        }
    }

    /// How a repository lands, as a Landing view's section names it
    /// (board 193), onto `branch`.
    pub fn lands_words(self, branch: &str) -> String {
        match self {
            Way::SwitchboardQueue => "lands by Switchboard's merge queue".into(),
            Way::GithubQueue => "lands by GitHub's merge queue".into(),
            Way::AutoMerge => "lands by GitHub's auto-merge".into(),
            Way::Direct => format!("lands straight on {branch}"),
            Way::Service => "lands by a merge service".into(),
        }
    }

    /// When a tab landing this way onto `branch` is done, as a Landing
    /// view's section says it (board 193).
    pub fn done_words(self, branch: &str) -> String {
        match self {
            Way::SwitchboardQueue => {
                format!("done when its PR merges on {branch}, then cleaned up")
            }
            Way::GithubQueue | Way::AutoMerge => {
                "done when GitHub merges its PR, then cleaned up".into()
            }
            Way::Direct => format!("done when its commit is pushed to {branch}, then cleaned up"),
            Way::Service => "done when the service merges its PR, then cleaned up".into(),
        }
    }

    /// The Landing view's columns after the first (the worktree, or the
    /// pull request), as board 193 heads them: the way's own steps, then
    /// landed and cleaned up, in every way.
    pub fn columns(self) -> &'static [&'static str] {
        match self {
            Way::SwitchboardQueue => &[
                "review",
                "checks",
                "queued",
                "train",
                "landed",
                "cleaned up",
            ],
            Way::GithubQueue => &[
                "review",
                "checks",
                "approved",
                "queued",
                "landed",
                "cleaned up",
            ],
            Way::AutoMerge => &["review", "checks", "auto-merge", "landed", "cleaned up"],
            Way::Direct => &[
                "tests",
                "review",
                "leak check",
                "lock",
                "landed",
                "cleaned up",
            ],
            Way::Service => &[
                "review",
                "checks",
                "handed over",
                "its queue",
                "landed",
                "cleaned up",
            ],
        }
    }

    /// What a tab does, when it is done, what it fixes, what it never does
    /// and what the way needs, as the menu shows them (board 192).
    pub fn facts(self) -> [Fact; 5] {
        match self {
            Way::SwitchboardQueue => [
                (
                    "a tab",
                    "reviews, pushes and opens its PR, and adds it to Switchboard's queue once its checks pass; the workspace tests it with the next few on a ci/train-* branch and squash-merges them in order",
                ),
                (
                    "done when",
                    "its PR merges with its train and its worktree is gone",
                ),
                (
                    "it fixes",
                    "a failing check, a conflict, or a train that failed for it, then queues it again",
                ),
                (
                    "never",
                    "brings a PR up to date with the base, or pushes to the default branch",
                ),
                (
                    "it needs",
                    "CI on a push to ci/train-*, and a base branch that doesn't require PRs to be up to date",
                ),
            ],
            Way::GithubQueue => [
                (
                    "a tab",
                    "reviews, pushes and opens its PR, and adds it to GitHub's queue once its checks pass; GitHub merges it",
                ),
                ("done when", "GitHub merges the PR and its worktree is gone"),
                (
                    "it fixes",
                    "a failing check or a drop from the queue, then queues it again",
                ),
                ("never", "merges by itself or pushes to the default branch"),
                ("it needs", "a merge queue on the default branch"),
            ],
            Way::AutoMerge => [
                (
                    "a tab",
                    "reviews, pushes and opens its PR and turns on auto-merge; GitHub merges it once its checks pass",
                ),
                ("done when", "GitHub merges the PR and its worktree is gone"),
                (
                    "it fixes",
                    "a failing check, a conflict, or a base it must be up to date with, by a rebase",
                ),
                ("never", "merges by itself or pushes to the default branch"),
                (
                    "it needs",
                    "auto-merge allowed in the repository's settings on GitHub",
                ),
            ],
            Way::Direct => [
                (
                    "a tab",
                    "takes the landing lock, tests, reviews and pushes onto the default branch, as its steps say",
                ),
                (
                    "done when",
                    "its push is on the default branch and its worktree is gone",
                ),
                (
                    "it fixes",
                    "a failing test or a rebase's conflict, before the push",
                ),
                ("never", "opens a PR or forces a push"),
                (
                    "it needs",
                    "push rights to the default branch, with no rule asking for a PR",
                ),
            ],
            Way::Service => [
                (
                    "a tab",
                    "reviews, pushes and opens its PR, and hands it to the service once its checks pass, by its label, its comment or both; the service merges it",
                ),
                (
                    "done when",
                    "the service merges the PR and its worktree is gone",
                ),
                (
                    "it fixes",
                    "a failing check, the service's own check among them, pushing the fix to the PR the service holds; a conflict it leaves to you",
                ),
                (
                    "never",
                    "rebases or queues the PR, merges it by itself or pushes to the default branch",
                ),
                (
                    "it needs",
                    "the label or comment the service takes a PR by and the check it reports, named in Settings",
                ),
            ],
        }
    }

    /// The steps a landing this way goes through, in order, as
    /// `switchboard landing` prints them, every way ending with the
    /// worktree cleaned up.
    pub fn steps(self) -> &'static [&'static str] {
        match self {
            Way::SwitchboardQueue => &[
                "review",
                "open-pr",
                "checks",
                "queue",
                "train",
                "merged",
                "cleaned-up",
            ],
            Way::GithubQueue => &[
                "review",
                "open-pr",
                "checks",
                "queue",
                "merged",
                "cleaned-up",
            ],
            Way::AutoMerge => &["review", "open-pr", "auto-merge", "merged", "cleaned-up"],
            Way::Direct => &["lock", "test", "review", "settle", "push", "cleaned-up"],
            Way::Service => &[
                "review",
                "open-pr",
                "checks",
                "hand-over",
                "service",
                "merged",
                "cleaned-up",
            ],
        }
    }

    /// When a tab landing this way is done, for the Repositories row.
    pub fn done_when(self) -> &'static str {
        self.facts()[1].1
    }
}

/// The reviews a direct landing can have.
pub const REVIEWS: [&str; 2] = ["review-loop", "none"];
/// Where a direct landing's tests can run: in the worktree, or in the
/// repository's CI.
pub const TEST_IN: [&str; 2] = ["here", "ci"];
/// How long GitHub's answer on whether each way is set up stands before
/// Settings asks again, in ms.
pub const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// A direct landing's steps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Steps {
    /// The tests' command; `None` when not set, so a landing runs what the
    /// repository's CLAUDE.md names, where an empty one is none.
    pub test: Option<String>,
    /// Where the tests run, one of `TEST_IN`.
    pub test_in: String,
    pub squash: bool,
    pub review: String,
    pub before_push: String,
    /// Run after a clean rebase in place of the tests; empty, the tests.
    pub after_rebase: String,
}

impl Default for Steps {
    fn default() -> Steps {
        Steps {
            test: None,
            test_in: "here".into(),
            squash: true,
            review: "review-loop".into(),
            before_push: String::new(),
            after_rebase: String::new(),
        }
    }
}

impl Steps {
    /// As JSON, with the file's names.
    pub fn json(&self) -> Value {
        json!({
            "test": self.test,
            "testIn": self.test_in,
            "squash": self.squash,
            "review": self.review,
            "beforePush": self.before_push,
            "afterRebase": self.after_rebase,
        })
    }
}

/// A merge service's hand-over and check: its `serviceLabel`, the label
/// added to a PR, and `serviceComment`, the comment posted on it (a bot's
/// command, such as `/merge`), at least one of them, and `serviceCheck`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Service {
    pub label: Option<String>,
    pub comment: Option<String>,
    pub check: String,
}

impl Service {
    /// As JSON, with the file's names cut to `label`, `comment` and
    /// `check`.
    pub fn json(&self) -> Value {
        json!({ "label": self.label, "comment": self.comment, "check": self.check })
    }

    /// How the service takes a PR, in words.
    pub fn hand_over(&self) -> String {
        match (&self.label, &self.comment) {
            (Some(l), Some(c)) => format!("the {l} label and the comment {c}"),
            (Some(l), None) => format!("the {l} label"),
            (None, Some(c)) => format!("the comment {c}"),
            (None, None) => unreachable!("a service has one"),
        }
    }
}

/// Whether a landing key holds a value it can.
fn valid(key: &str, value: &Value) -> bool {
    match key {
        "lands" => value.as_str().and_then(Way::parse).is_some(),
        "review" => value.as_str().is_some_and(|v| REVIEWS.contains(&v)),
        "testIn" => value.as_str().is_some_and(|v| TEST_IN.contains(&v)),
        "test" | "beforePush" | "afterRebase" => value.is_string(),
        "serviceLabel" => value.as_str().is_some_and(label_ok),
        "serviceComment" => value.as_str().is_some_and(label_ok),
        "serviceCheck" => value.as_str().is_some_and(label_ok),
        "squash" => value.is_boolean(),
        _ => false,
    }
}

/// A repository's landing keys, only those holding a value they can.
pub fn landing_entry(data: &Map<String, Value>, repo: &str) -> Map<String, Value> {
    entry_of(data, repo)
        .into_iter()
        .filter(|(k, v)| valid(k, v))
        .collect()
}

/// An entry's way, when it sets one it can.
pub fn lands_of(entry: &Map<String, Value>) -> Option<Way> {
    entry
        .get("lands")
        .and_then(Value::as_str)
        .and_then(Way::parse)
}

/// An entry's merge service, when it names its check and its label, its
/// comment or both.
pub fn service_of(entry: &Map<String, Value>) -> Option<Service> {
    let entry: Map<String, Value> = entry
        .iter()
        .filter(|(k, v)| valid(k, v))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let text = |k: &str| {
        entry
            .get(k)
            .and_then(Value::as_str)
            .map(|s| s.trim().to_string())
    };
    let label = text("serviceLabel");
    let comment = text("serviceComment");
    if label.is_none() && comment.is_none() {
        return None;
    }
    Some(Service {
        label,
        comment,
        check: text("serviceCheck")?,
    })
}

/// A direct landing's steps from an entry's keys, the defaults for the rest.
pub fn steps_of(entry: &Map<String, Value>) -> Steps {
    let text = |k: &str| entry.get(k).and_then(Value::as_str).map(str::to_string);
    let d = Steps::default();
    Steps {
        test: text("test"),
        test_in: text("testIn").unwrap_or(d.test_in),
        squash: entry
            .get("squash")
            .and_then(Value::as_bool)
            .unwrap_or(d.squash),
        review: text("review").unwrap_or(d.review),
        before_push: text("beforePush").unwrap_or(d.before_push),
        after_rebase: text("afterRebase").unwrap_or(d.after_rebase),
    }
}

/// Sets one landing key, or `tickets` or `issueLabel`, of a repository's
/// entry, keeping the rest of it; `lands` and `tickets` `unset`, and an
/// empty `issueLabel`, `serviceLabel`, `serviceComment` or `serviceCheck`,
/// take the key out,
/// and an entry left empty goes. Never over a file that can't be read as a
/// JSON object, the version before kept as `repos.json.bak`.
pub fn set_repo(repo: &str, key: &str, value: Value) -> Result<(), String> {
    set_repo_in(&file(), repo, key, value)
}

/// `set_repo` on the file at `path`.
pub fn set_repo_in(path: &Path, repo: &str, key: &str, value: Value) -> Result<(), String> {
    let unset = (["lands", "tickets"].contains(&key) && value.as_str() == Some("unset"))
        || ([
            "issueLabel",
            "serviceLabel",
            "serviceComment",
            "serviceCheck",
        ]
        .contains(&key)
            && value.as_str().is_some_and(|v| v.trim().is_empty()));
    let tickets_ok = (key == "tickets"
        && value
            .as_str()
            .and_then(crate::tickets::Kept::parse)
            .is_some())
        || (key == "issueLabel" && value.as_str().is_some_and(label_ok));
    if !unset && !tickets_ok && !valid(key, &value) {
        return Err(
            if [
                "tickets",
                "issueLabel",
                "lands",
                "serviceLabel",
                "serviceComment",
                "serviceCheck",
                "test",
                "testIn",
                "squash",
                "review",
                "beforePush",
                "afterRebase",
            ]
            .contains(&key)
            {
                format!("not a value for {key}: {value}")
            } else {
                format!("not a landing setting: {key}")
            },
        );
    }
    let value = match value {
        Value::String(s) if key.starts_with("service") => Value::String(s.trim().to_string()),
        v => v,
    };
    // A merge service keeps a label or a comment to hand a PR to it.
    let other_hand_over = match key {
        "serviceLabel" => Some("serviceComment"),
        "serviceComment" => Some("serviceLabel"),
        _ => None,
    };
    if unset && let Some(other) = other_hand_over {
        let entry = entry_of(&read_file(path).unwrap_or_default(), repo);
        let has_other = entry
            .get(other)
            .and_then(Value::as_str)
            .is_some_and(|v| !v.trim().is_empty());
        if lands_of(&entry) == Some(Way::Service) && !has_other {
            return Err(
                "a merge service needs the label or the comment that hands it a PR".to_string(),
            );
        }
    }
    change_file(path, |data| {
        // The entry under the name the file has it by, in whatever case.
        let name = data
            .keys()
            .find(|k| k.eq_ignore_ascii_case(repo))
            .cloned()
            .unwrap_or_else(|| repo.to_string());
        let mut entry = data
            .get(&name)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        if unset {
            entry.remove(key);
        } else {
            entry.insert(key.to_string(), value);
        }
        if entry.is_empty() {
            data.remove(&name);
        } else {
            data.insert(name, Value::Object(entry));
        }
    })
}

/// Writes repos.json at `path` as `change` leaves it, never over a file
/// that can't be read as a JSON object, the version before kept as
/// `repos.json.bak`.
fn change_file(path: &Path, change: impl FnOnce(&mut Map<String, Value>)) -> Result<(), String> {
    let mut data = read_file(path).map_err(|e| {
        format!(
            "{} can't be read ({e}); fix it by hand first",
            paths::tilde(path)
        )
    })?;
    change(&mut data);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    if path.exists() {
        std::fs::copy(path, bak_of(path)).map_err(|e| e.to_string())?;
    }
    util::write_atomic(
        path,
        format!("{}\n", util::json_pretty(&Value::Object(data))),
    )
    .map_err(|e| e.to_string())
}

/// `repos.json.bak` beside the file at `path`.
fn bak_of(path: &Path) -> PathBuf {
    let mut bak = path.as_os_str().to_owned();
    bak.push(".bak");
    PathBuf::from(bak)
}

/// How a repository lands, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Landing {
    /// `owner/name` on GitHub, when its origin is there.
    pub repo: Option<String>,
    /// What it is known by here: `owner/name`, else its git directory.
    pub key: String,
    pub lands: Way,
    pub why: String,
    /// Where the way came from: `repos.json`, or `branch` (the way the
    /// branch started landing with, kept on it).
    pub source: String,
    /// The way repos.json sets now, when the branch keeps another.
    pub set: Option<Way>,
    pub steps: Steps,
    /// The merge service's hand-over and check, for `service`.
    pub service: Option<Service>,
}

/// The words a landing stops with while a repository's `lands` isn't set,
/// `name` its `owner/name` or what stands for it, naming the row of
/// Settings that sets it.
pub fn not_set_words(name: &str) -> String {
    format!(
        "{name} lands nothing yet: choose how its work lands in Settings' Repositories, on its {name} row, or set \"lands\" for it in {}",
        crate::files::shown("repos.json")
    )
}

/// The git config key a branch keeps the way it started landing with.
fn recorded_key(branch: &str) -> String {
    format!("branch.{branch}.wtnLands")
}

/// The branch checked out at `dir`.
fn branch_at(dir: &Path) -> Option<String> {
    worktree::git_at(dir, &["rev-parse", "--abbrev-ref", "HEAD"])
        .ok()
        .filter(|b| !b.is_empty() && b != "HEAD")
}

/// The way the branch at `dir` started landing with, as kept on it.
pub fn recorded_at(dir: &Path) -> Option<Way> {
    let branch = branch_at(dir)?;
    worktree::git_at(dir, &["config", "--get", &recorded_key(&branch)])
        .ok()
        .and_then(|w| Way::parse(w.trim()))
}

/// Keeps `way` on the branch at `dir` as the way it started landing with,
/// unless it keeps one already; what it keeps after. Runs git.
pub fn record_at(dir: &Path, way: Way) -> Result<Way, String> {
    if let Some(kept) = recorded_at(dir) {
        return Ok(kept);
    }
    let branch = branch_at(dir).ok_or("no branch is checked out")?;
    worktree::git_at(dir, &["config", &recorded_key(&branch), way.key()])
        .map_err(|e| e.to_string())?;
    Ok(way)
}

/// How the repository at `dir` lands by what repos.json sets, read from
/// the files alone with no git process (origin from git's config file), for
/// a read on the server's task; `None` while it isn't set.
pub fn lands_known(dir: &Path) -> Option<Way> {
    let data = read_safely();
    keys_known(dir).find_map(|k| lands_of(&entry_of(&data, &k)))
}

/// How a repository lands by its key (`owner/name`, else its shared git
/// directory), as repos.json sets it, for a read that runs on every tick.
pub fn lands_by_key(key: &str) -> Option<Way> {
    lands_of(&entry_of(&read_safely(), key))
}

/// How the repository at `dir` lands, as the module's comment says: the
/// way the branch started landing with, else what repos.json sets; an
/// error, naming Settings' row, while neither says. Runs git, so off the
/// server's task. Reads alone: an old `landing` with no `lands` lands
/// nothing here (`landing_carrying` carries it over).
pub fn landing_for(dir: &Path) -> Result<Landing, String> {
    landing_with(dir, &file(), None)
}

/// `landing_for` for the landing and for starting work that lands
/// (`switchboard landing`, which queue-and-clean reads the way from,
/// autopilot's start and pick checks, the work box's branch check,
/// `wt --target`): a repository whose
/// entry still has `landing` and no `lands`, with no way kept on the
/// branch, gets it carried over then, as the migration would, so a
/// migration whose change didn't stay in the file never strands it. May
/// ask GitHub, so off the server's task.
pub fn landing_carrying(dir: &Path) -> Result<Landing, String> {
    landing_with(dir, &file(), Some(&old_settings))
}

/// `landing_for` with repos.json at `repos`, `old` saying how the old
/// settings stand for carrying an old `landing` over (`carry_over_in`),
/// `None` to read alone.
pub fn landing_with(
    dir: &Path,
    repos: &Path,
    old: Option<&OldSettings>,
) -> Result<Landing, String> {
    let (repo, _, key) =
        repository_at(dir).ok_or_else(|| format!("{} isn't in a git repository", dir.display()))?;
    // A repos.json that can't be read stops it, rather than landing without
    // what it may say.
    let data = read_file(repos)
        .map_err(|e| format!("{} can't be read ({e}); fix it first", paths::tilde(repos)))?;
    let mut name = key.clone();
    if landing_entry(&data, &key).is_empty()
        && !entry_of(&data, &key).contains_key("landing")
        && let Some(configured) = configured_at(dir)
    {
        name = configured;
    }
    let mut entry = landing_entry(&data, &name);
    let old_landing = entry_of(&data, &name)
        .get("landing")
        .and_then(Value::as_str)
        .map(str::to_string);
    let recorded = recorded_at(dir);
    if lands_of(&entry).is_none()
        && recorded.is_none()
        && let Some(landing) = old_landing
        && let Some(old) = old
    {
        let not_set = not_set_words(repo.as_deref().unwrap_or(&key));
        let (queue_was_on, setup) = old(&name, &landing).map_err(|e| format!("{not_set} ({e})"))?;
        carry_over_in(repos, &name, queue_was_on, setup.as_ref())
            .map_err(|e| format!("{not_set} ({e})"))?;
        let data = read_file(repos)
            .map_err(|e| format!("{} can't be read ({e}); fix it first", paths::tilde(repos)))?;
        entry = landing_entry(&data, &name);
    }
    let set = lands_of(&entry);
    let (lands, why, source) = match (recorded, set) {
        (Some(kept), Some(now)) if kept != now => (
            kept,
            format!(
                "this branch started landing by {}, which it finishes; the next landing goes by {}",
                kept.label(),
                now.label()
            ),
            "branch",
        ),
        (Some(kept), None) => (
            kept,
            "this branch started landing this way, which it finishes".to_string(),
            "branch",
        ),
        (_, Some(now)) => (
            now,
            "set in Settings' Repositories".to_string(),
            "repos.json",
        ),
        (None, None) => {
            return Err(not_set_words(repo.as_deref().unwrap_or(&key)));
        }
    };
    Ok(Landing {
        repo,
        key,
        lands,
        why,
        source: source.into(),
        set,
        steps: steps_of(&entry),
        service: service_of(&entry),
    })
}

// ── Whether each way is set up ──────────────────────────────────────────────

/// What GitHub said of a repository, as each way's check reads it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Setup {
    /// When it was asked, in ms.
    pub at: i64,
    /// The default branch.
    pub branch: Option<String>,
    /// The default branch has a merge queue.
    pub merge_queue: bool,
    /// The repository allows auto-merge.
    pub auto_merge: bool,
    /// You can push to the default branch.
    pub can_push: bool,
    /// A rule makes the default branch take a pull request.
    pub requires_pr: bool,
    /// The default branch requires PRs to be up to date.
    pub strict: bool,
    /// A workflow of the default branch runs on a push to `ci/train-*`.
    pub train_ci: bool,
    /// What a merge service's check reads: `None` in an answer kept by a
    /// build from before it was asked.
    pub names: Option<Names>,
}

/// The repository's labels and the checks reported on its most recently
/// updated PRs (`RECENT_PRS`), for a merge service's check.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Names {
    pub labels: Vec<String>,
    pub checks: Vec<String>,
}

/// How many of a repository's most recently updated PRs a merge service's
/// check is looked for on.
pub const RECENT_PRS: usize = 20;

impl Setup {
    fn json(&self) -> Value {
        json!({
            "at": util::js_number(self.at as f64),
            "branch": self.branch,
            "mergeQueue": self.merge_queue,
            "autoMerge": self.auto_merge,
            "canPush": self.can_push,
            "requiresPr": self.requires_pr,
            "strict": self.strict,
            "trainCi": self.train_ci,
            "names": self.names.as_ref().map(|n| json!({ "labels": n.labels, "checks": n.checks })),
        })
    }

    fn of(v: &Value) -> Option<Setup> {
        let on = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
        Some(Setup {
            at: v.get("at")?.as_f64()? as i64,
            branch: v.get("branch").and_then(Value::as_str).map(str::to_string),
            merge_queue: on("mergeQueue"),
            auto_merge: on("autoMerge"),
            can_push: on("canPush"),
            requires_pr: on("requiresPr"),
            strict: on("strict"),
            train_ci: on("trainCi"),
            names: v.get("names").filter(|n| n.is_object()).map(|n| {
                let list = |k: &str| {
                    n.get(k)
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                };
                Names {
                    labels: list("labels"),
                    checks: list("checks"),
                }
            }),
        })
    }

    /// The way GitHub's rules suggest, which only you choose: GitHub's
    /// merge queue where there is one, straight on the branch where nothing
    /// asks for a PR and you can push, else auto-merge where it is allowed,
    /// else Switchboard's merge queue where its CI runs on trains.
    pub fn suggests(&self) -> Option<Way> {
        if self.merge_queue {
            Some(Way::GithubQueue)
        } else if !self.requires_pr && self.can_push {
            Some(Way::Direct)
        } else if self.auto_merge {
            Some(Way::AutoMerge)
        } else if self.train_ci && !self.strict {
            Some(Way::SwitchboardQueue)
        } else {
            None
        }
    }
}

/// `repos-setup.json` in the state directory: what GitHub said of each
/// repository when Settings last asked, by its key.
pub fn setup_file() -> PathBuf {
    paths::state_dir().join("repos-setup.json")
}

/// What GitHub said of a repository when last asked, however long ago.
pub fn setup_in(file: &Path, key: &str) -> Option<Setup> {
    let data = util::read_json(file)?;
    data.as_object()?
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .and_then(|(_, v)| Setup::of(v))
}

/// Keeps what GitHub said of a repository.
pub fn remember_setup(file: &Path, key: &str, setup: &Setup) {
    let mut data = util::read_json_object(file);
    data.insert(key.to_string(), setup.json());
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = util::write_atomic(file, util::json_compact(&Value::Object(data)));
}

/// Whether a way is set up: `Some(true)` with what shows it, `Some(false)`
/// with what is missing, `None` with why it couldn't be told.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub ok: Option<bool>,
    pub words: String,
}

/// Whether `way` is set up for a repository, from `setup` (GitHub's
/// answer, `None` while there is none) and its entry in repos.json.
pub fn check(way: Way, setup: Option<&Setup>, entry: &Map<String, Value>) -> Check {
    let yes = |w: String| Check {
        ok: Some(true),
        words: w,
    };
    let no = |w: String| Check {
        ok: Some(false),
        words: w,
    };
    let service = service_of(entry);
    if way == Way::Service && service.is_none() {
        return no("the label or comment and the check the service uses aren't named".into());
    }
    let Some(s) = setup else {
        return Check {
            ok: None,
            words: if settings::is_on("github") {
                "not checked yet: Settings asks GitHub while it is open".into()
            } else {
                "not checked: the github setting is off".into()
            },
        };
    };
    let branch = s.branch.as_deref().unwrap_or("the default branch");
    match way {
        Way::SwitchboardQueue => match (s.train_ci, s.strict) {
            (true, false) => yes("CI runs on ci/train-* branches".into()),
            (false, _) => no(format!(
                "no workflow on {branch} runs on a push to ci/train-*"
            )),
            (true, true) => no(format!("{branch} requires PRs to be up to date")),
        },
        Way::GithubQueue if s.merge_queue => yes(format!("{branch} has a merge queue on GitHub")),
        Way::GithubQueue => no(format!("{branch} has no merge queue on GitHub")),
        Way::AutoMerge if s.auto_merge => yes("GitHub allows auto-merge here".into()),
        Way::AutoMerge => no("auto-merge isn't allowed in its settings on GitHub".into()),
        Way::Direct => match (s.can_push, s.requires_pr) {
            (true, false) => yes(format!("you can push to {branch}; no rule asks for a PR")),
            (false, _) => no(format!("you can't push to {branch}")),
            (true, true) => no(format!("a rule asks for a PR on {branch}")),
        },
        Way::Service => {
            let service = service.expect("answered above");
            let Some(names) = &s.names else {
                return Check {
                    ok: None,
                    words: "not checked yet: Settings asks GitHub while it is open".into(),
                };
            };
            if let Some(label) = &service.label
                && !names.labels.iter().any(|l| l.eq_ignore_ascii_case(label))
            {
                return no(format!("the repository has no {label} label on GitHub"));
            }
            if !names.checks.contains(&service.check) {
                return no(format!(
                    "{} isn't reported on any of its last {RECENT_PRS} PRs",
                    service.check
                ));
            }
            yes(format!(
                "{} and {}'s check, seen on its recent PRs",
                service.hand_over(),
                service.check
            ))
        }
    }
}

/// What GitHub says of a repository, for each way's check: its default
/// branch's merge queue, auto-merge, whether you can push and whether a
/// rule asks for a PR (branch protection asking for one, a review count at
/// all, 0 included, or no push for you, or a ruleset with a pull request or
/// merge queue rule), whether PRs must be up to date, and whether a
/// workflow runs on a push to `ci/train-*`. A repository on a plan without
/// rulesets answers 404, which is none; any other failure is a failure to
/// find out. Asks GitHub, so off the server's task.
pub fn ask_setup(repo: &str, now: i64) -> Result<Setup, String> {
    let (owner, name) = repo.split_once('/').ok_or("not owner/name")?;
    let source = "settings: how a repository lands";
    let data = wait_for(github::graphql(
        "query($o: String!, $n: String!, $prs: Int!) { repository(owner: $o, name: $n) { autoMergeAllowed viewerPermission mergeQueue { url } defaultBranchRef { name refUpdateRule { viewerCanPush requiredApprovingReviewCount } } object(expression: \"HEAD:.github/workflows\") { ... on Tree { entries { name object { ... on Blob { text } } } } } labels(first: 100) { nodes { name } } pullRequests(first: $prs, orderBy: { field: UPDATED_AT, direction: DESC }) { nodes { commits(last: 1) { nodes { commit { statusCheckRollup { contexts(first: 100) { nodes { ... on CheckRun { name } ... on StatusContext { context } } } } } } } } } } }",
        json!({ "o": owner, "n": name, "prs": RECENT_PRS }),
        source,
    ))
    .map_err(|e| e.to_string())?;
    // Branch protection's "up to date" is readable only by some; one that
    // can't be read is taken as off, and the queue says so if it is on.
    let strict_classic = wait_for(github::graphql_partial(
        "query($o: String!, $n: String!) { repository(owner: $o, name: $n) { defaultBranchRef { branchProtectionRule { requiresStrictStatusChecks } } } }",
        json!({ "o": owner, "n": name }),
        source,
    ))
    .ok()
    .and_then(|(v, _)| {
        v.pointer("/repository/defaultBranchRef/branchProtectionRule/requiresStrictStatusChecks")
            .and_then(Value::as_bool)
    })
    .unwrap_or(false);
    let mut setup = setup_of(&data, now, &|branch: &str| {
        let path = format!(
            "repos/{owner}/{name}/rules/branches/{}",
            branch.replace('/', "%2F")
        );
        match wait_for(github::rest("GET", &path, None, source)) {
            Ok(v) => Ok(v),
            Err(e)
                if e.downcast_ref::<crate::net::ServiceError>()
                    .is_some_and(|s| s.kind == crate::net::ErrorKind::NotFound) =>
            {
                Ok(Value::Array(Vec::new()))
            }
            Err(e) => Err(e.to_string()),
        }
    })?;
    setup.strict |= strict_classic;
    Ok(setup)
}

/// GitHub's answer for a repository read as `ask_setup` reads it, its
/// branch's rulesets asked of `rules`.
pub fn setup_of(
    data: &Value,
    now: i64,
    rules: &dyn Fn(&str) -> Result<Value, String>,
) -> Result<Setup, String> {
    let repository = data
        .get("repository")
        .filter(|r| r.is_object())
        .ok_or("GitHub didn't answer")?;
    let branch = repository
        .pointer("/defaultBranchRef/name")
        .and_then(Value::as_str)
        .map(str::to_string);
    let rule = repository.pointer("/defaultBranchRef/refUpdateRule");
    // With no protection on the branch GitHub gives no rule, and then your
    // permission on the repository says whether you can push.
    let can_push = rule
        .and_then(|r| r.get("viewerCanPush"))
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            matches!(
                repository.get("viewerPermission").and_then(Value::as_str),
                Some("ADMIN" | "MAINTAIN" | "WRITE")
            )
        });
    let reviews = rule.is_some_and(|r| {
        r.get("requiredApprovingReviewCount")
            .is_some_and(Value::is_number)
    });
    let mut setup = Setup {
        at: now,
        branch: branch.clone(),
        merge_queue: repository.get("mergeQueue").is_some_and(Value::is_object),
        auto_merge: repository.get("autoMergeAllowed") == Some(&Value::Bool(true)),
        can_push,
        requires_pr: reviews || !can_push,
        strict: false,
        train_ci: false,
        names: Some(names_of(repository)),
    };
    setup.requires_pr |= setup.merge_queue;
    if let Some(b) = branch.as_deref() {
        for r in rules(b)?.as_array().into_iter().flatten() {
            match r.get("type").and_then(Value::as_str) {
                Some("pull_request") => setup.requires_pr = true,
                Some("merge_queue") => {
                    setup.requires_pr = true;
                    setup.merge_queue = true;
                }
                Some("required_status_checks") => {
                    setup.strict |= r
                        .pointer("/parameters/strict_required_status_checks_policy")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                }
                _ => {}
            }
        }
    }
    setup.train_ci = repository
        .pointer("/object/entries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|e| {
            e.get("name")
                .and_then(Value::as_str)
                .is_some_and(|n| n.ends_with(".yml") || n.ends_with(".yaml"))
        })
        .filter_map(|e| e.pointer("/object/text").and_then(Value::as_str))
        .any(runs_on_trains);
    Ok(setup)
}

/// The labels and the checks on the recent PRs in GitHub's answer for a
/// repository, each check once.
fn names_of(repository: &Value) -> Names {
    let nodes = |v: Option<&Value>| -> Vec<Value> {
        v.and_then(Value::as_array).cloned().unwrap_or_default()
    };
    let labels = nodes(repository.pointer("/labels/nodes"))
        .iter()
        .filter_map(|l| l.get("name")?.as_str().map(str::to_string))
        .collect();
    let mut checks: Vec<String> = Vec::new();
    for pr in nodes(repository.pointer("/pullRequests/nodes")) {
        for commit in nodes(pr.pointer("/commits/nodes")) {
            let contexts = commit.pointer("/commit/statusCheckRollup/contexts/nodes");
            for c in nodes(contexts) {
                let name = c.get("name").or_else(|| c.get("context"));
                if let Some(name) = name.and_then(Value::as_str)
                    && !checks.iter().any(|k| k == name)
                {
                    checks.push(name.to_string());
                }
            }
        }
    }
    Names { labels, checks }
}

/// Whether a workflow's text runs it on a push to a merge queue train's
/// branch, `ci/train-*`, or one taking every `ci/` branch.
pub fn runs_on_trains(text: &str) -> bool {
    text.contains("ci/train-") || text.contains("ci/train*") || text.contains("ci/**")
}

// ── Carrying the old settings over ──────────────────────────────────────────

/// A repository and the way carrying the old settings over gave it.
pub type WaySet = (String, Way);

/// The state directory's mark that `migrate_lands` ran, so it runs once.
pub fn migrated_mark() -> PathBuf {
    paths::state_dir().join("repos-lands-migrated")
}

/// How the old settings stand for a repository whose entry still has
/// `landing` (its key, and that `landing`): whether `pr_merge_queue` was
/// on, and what GitHub says of it (`None` where it isn't needed or GitHub
/// knows no such repository); an error when GitHub couldn't be asked.
pub type OldSettings = dyn Fn(&str, &str) -> Result<(bool, Option<Setup>), String>;

/// Whether `pr_merge_queue` was on, from this computer's settings
/// (`setting`, while it still holds it) and the migration's mark (`mark`,
/// `None` when there is none): the mark's `queueWasOn`; with a mark of an
/// earlier build, which kept no `queueWasOn` and took the setting out,
/// whether GitHub says the repository's CI runs on Switchboard's trains,
/// which only its queue pushes; off with no mark and no setting, as the
/// migration reads it.
pub fn queue_was_on(setting: Option<&Value>, mark: Option<&Value>, setup: Option<&Setup>) -> bool {
    if let Some(on) = setting {
        return on == &Value::Bool(true);
    }
    match mark.map(|m| m.get("queueWasOn")) {
        Some(Some(on)) => on == &Value::Bool(true),
        Some(None) => setup.is_some_and(|s| s.train_ci),
        None => false,
    }
}

/// `OldSettings` on this computer: `queue_was_on` from its settings and
/// the migration's mark; GitHub's answer as last remembered, else asked now
/// with the github setting on. Asks GitHub, so off the server's task.
pub fn old_settings(repo: &str, landing: &str) -> Result<(bool, Option<Setup>), String> {
    let setting = util::read_json(paths::local_config_dir().join("settings.json"))
        .and_then(|v| v.get("pr_merge_queue").cloned());
    // A mark of an earlier build holds a bare number, read as no object.
    let mark = std::fs::read_to_string(migrated_mark()).ok().map(|text| {
        serde_json::from_str::<Value>(&text)
            .ok()
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}))
    });
    let with = |setup: Option<Setup>| {
        Ok((
            queue_was_on(setting.as_ref(), mark.as_ref(), setup.as_ref()),
            setup,
        ))
    };
    if landing == "direct" || github::parse_name_with_owner(repo).is_none() {
        return with(None);
    }
    if let Some(known) = setup_in(&setup_file(), repo) {
        return with(Some(known));
    }
    if !settings::is_on("github") {
        return Err("the github setting is off, so how it can land can't be told".into());
    }
    match ask_setup(repo, util::now_ms()) {
        Ok(s) => {
            remember_setup(&setup_file(), repo, &s);
            with(Some(s))
        }
        Err(e) if gone(&e) => with(None),
        Err(e) => Err(format!("GitHub didn't say how it can land: {e}")),
    }
}

/// Carries the old `landing` of the entry under `name` in the repos.json
/// at `path` over to `lands`, as `lands_migrated` would, leaving every
/// other entry; the way it set, `None` when the entry needed none.
pub fn carry_over_in(
    path: &Path,
    name: &str,
    queue_was_on: bool,
    setup: Option<&Setup>,
) -> Result<Option<Way>, String> {
    let mut way = None;
    change_file(path, |d| {
        let Some((k, v)) = d
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(k, v)| (k.clone(), v.clone()))
        else {
            return;
        };
        let one = Map::from_iter([(k.clone(), v)]);
        if let Some((mut out, set)) = lands_migrated(&one, &[], queue_was_on, &|_| setup.cloned())
            && let Some(entry) = out.shift_remove(&k)
        {
            d.insert(k, entry);
            way = set.first().map(|(_, w)| *w);
        }
    })?;
    Ok(way)
}

/// Writes the migration's mark at `mark`, keeping whether
/// `pr_merge_queue` was on, only once the repos.json at `path`, read
/// again, has a `lands` for every repository in `said`; the ones it
/// lacks otherwise, so the next start tries again.
pub fn mark_once_landed(
    path: &Path,
    mark: &Path,
    said: &[WaySet],
    queue_was_on: bool,
) -> Result<(), Vec<String>> {
    let data =
        read_file(path).map_err(|_| said.iter().map(|(r, _)| r.clone()).collect::<Vec<_>>())?;
    let missing: Vec<String> = said
        .iter()
        .filter(|(repo, _)| lands_of(&entry_of(&data, repo)).is_none())
        .map(|(repo, _)| repo.clone())
        .collect();
    if !missing.is_empty() {
        return Err(missing);
    }
    if let Some(dir) = mark.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = util::write_atomic(
        mark,
        format!(
            "{}\n",
            json!({ "at": util::js_number(util::now_ms() as f64), "queueWasOn": queue_was_on })
        ),
    );
    Ok(())
}

/// The way the old settings come to for one repository: `direct` where its
/// `landing` was `direct`; through a PR otherwise, by Switchboard's merge
/// queue where `pr_merge_queue` was on and GitHub says its CI runs on
/// `ci/train-*`, by GitHub's merge queue where its default branch has one,
/// else by auto-merge.
pub fn way_of_old(old: Option<&str>, queue_was_on: bool, setup: Option<&Setup>) -> Way {
    match (old, setup) {
        (Some("direct"), _) => Way::Direct,
        (_, Some(s)) if queue_was_on && s.train_ci && !s.merge_queue => Way::SwitchboardQueue,
        (_, Some(s)) if s.merge_queue => Way::GithubQueue,
        _ => Way::AutoMerge,
    }
}

/// What carrying `landing` and `pr_merge_queue` over to `lands` does to
/// repos.json: every entry with no `lands`, and each repository the old
/// guess had worked out (`guessed`, its key and what it found, which counts
/// as its `landing`), gets the way `way_of_old` gives, `setup` saying what
/// GitHub says of it; `landing` is dropped from every entry. A `lands`
/// already set stays. What it set, by repository, in the file's order;
/// `None` when it changes nothing.
pub fn lands_migrated(
    data: &Map<String, Value>,
    guessed: &[(String, String)],
    queue_was_on: bool,
    setup: &dyn Fn(&str) -> Option<Setup>,
) -> Option<(Map<String, Value>, Vec<WaySet>)> {
    let mut out = data.clone();
    for (key, _) in guessed {
        if !out.keys().any(|k| k.eq_ignore_ascii_case(key)) {
            out.insert(key.clone(), json!({}));
        }
    }
    let mut set = Vec::new();
    let mut changed = false;
    for (repo, entry) in out.iter_mut() {
        let Some(entry) = entry.as_object_mut() else {
            continue;
        };
        let old = entry.shift_remove("landing");
        changed |= old.is_some();
        if entry.contains_key("lands") {
            continue;
        }
        let worked_out = guessed
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(repo))
            .map(|(_, l)| l.as_str());
        let old = old.as_ref().and_then(Value::as_str).or(worked_out);
        let way = way_of_old(old, queue_was_on, setup(repo).as_ref());
        entry.insert("lands".into(), json!(way.key()));
        set.push((repo.clone(), way));
        changed = true;
    }
    changed.then_some((out, set))
}

/// While the rest of the carrying over waits on GitHub, a repository whose
/// `landing` was `direct` (set, or as the old guess found it) gets
/// `direct` at once, since that needs no answer of GitHub's; the rest
/// wait, `landing` kept for them.
fn direct_now(path: &Path, data: &Map<String, Value>, guessed: &[(String, String)]) {
    let guessed_direct = |repo: &str| {
        guessed
            .iter()
            .any(|(k, l)| k.eq_ignore_ascii_case(repo) && l == "direct")
    };
    let direct = |repo: &str, e: &Value| {
        e.get("lands").is_none()
            && match e.get("landing").and_then(Value::as_str) {
                Some(old) => old == "direct",
                None => guessed_direct(repo),
            }
    };
    let new_entry = |d: &Map<String, Value>| {
        guessed
            .iter()
            .any(|(k, l)| l == "direct" && !d.keys().any(|x| x.eq_ignore_ascii_case(k)))
    };
    if !new_entry(data) && !data.iter().any(|(k, e)| direct(k, e)) {
        return;
    }
    let _ = change_file(path, |d| {
        for (k, l) in guessed {
            if l == "direct" && !d.keys().any(|x| x.eq_ignore_ascii_case(k)) {
                d.insert(k.clone(), json!({}));
            }
        }
        for (repo, entry) in d.iter_mut() {
            if direct(repo, entry)
                && let Some(e) = entry.as_object_mut()
            {
                e.shift_remove("landing");
                e.insert("lands".into(), json!(Way::Direct.key()));
            }
        }
    });
}

/// Whether GitHub's failure to answer for a repository says it isn't there
/// for you: no repository in its answer, one GitHub can't resolve, or one an
/// organization's sign-in or the token's grants keep it out of.
fn gone(error: &str) -> bool {
    [
        "didn't answer",
        "Could not resolve to a Repository",
        "SAML",
        "Resource not accessible",
    ]
    .iter()
    .any(|w| error.contains(w))
}

/// Carries repos.json's `landing`, the old guess kept in the state
/// directory's `repos-landing.json` and Settings' `pr_merge_queue` over to
/// `lands`, once, as a new build's server first starts (`lands_migrated`),
/// the file before kept as `repos.json.bak`, and drops `pr_merge_queue`
/// from this computer's settings and the synced ones and the old guess.
/// GitHub is asked of each repository on it (`ask_setup`) with the github
/// setting on; when asking fails for one that isn't gone, nothing changes
/// and the next start tries again, so a moment offline never settles a way. What it says it
/// did, in one message; `None` when there was nothing to carry over or it
/// waits to try again. Asks GitHub, so off the server's task.
pub fn migrate_lands() -> Option<String> {
    let mark = migrated_mark();
    if mark.exists() {
        return None;
    }
    let setting_files = [
        crate::paths::local_config_dir().join("settings.json"),
        crate::settings::settings_file(),
    ];
    // A computer's setting: only this computer's own file counted.
    let queue_was_on = util::read_json(&setting_files[0])
        .and_then(|v| v.get("pr_merge_queue").cloned())
        == Some(Value::Bool(true));
    let guess_file = paths::state_dir().join("repos-landing.json");
    let guessed: Vec<(String, String)> = util::read_json_object(&guess_file)
        .into_iter()
        .filter_map(|(k, v)| Some((k, v.get("landing")?.as_str()?.to_string())))
        .collect();
    let path = file();
    // A file that can't be read is left, and tried again next start.
    let data = match read_file(&path) {
        Ok(data) => data,
        Err(e) => {
            eprintln!("repos.json: can't carry landing over to lands: {e}");
            return None;
        }
    };
    let mut asked = std::collections::HashMap::new();
    // With GitHub not to be asked, how a repository can land through a PR
    // can't be told, so all but `direct` waits until it can.
    if !settings::is_on("github") {
        direct_now(&path, &data, &guessed);
        return None;
    }
    if settings::is_on("github") {
        let now = util::now_ms();
        // Only those whose way hangs on GitHub's answer: no way set yet,
        // and not direct, set or guessed.
        let needs_answer = |repo: &str| {
            let entry = entry_of(&data, repo);
            let old = entry.get("landing").and_then(Value::as_str).or_else(|| {
                guessed
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(repo))
                    .map(|(_, l)| l.as_str())
            });
            !entry.contains_key("lands") && old != Some("direct")
        };
        let repos: Vec<&String> = data
            .keys()
            .chain(guessed.iter().map(|(k, _)| k))
            .filter(|r| github::parse_name_with_owner(r).is_some() && needs_answer(r))
            .collect();
        let mut failed = 0;
        for repo in &repos {
            match ask_setup(repo, now) {
                Ok(s) => {
                    remember_setup(&setup_file(), repo, &s);
                    asked.insert(repo.to_lowercase(), s);
                }
                // A repository GitHub says isn't there (gone, renamed, or
                // out of the token's reach) is carried over as if it weren't
                // asked; any other failure may be a moment offline.
                Err(e) if gone(&e) => {
                    eprintln!("repos.json: GitHub knows no {repo}: {e}");
                }
                Err(e) => {
                    failed += 1;
                    eprintln!("repos.json: GitHub didn't say how {repo} can land: {e}");
                }
            }
        }
        // A failure to ask is tried again next start, so a moment offline
        // never settles a way.
        if failed > 0 {
            direct_now(&path, &data, &guessed);
            return None;
        }
    }
    // Worked out on the file as it is now, after GitHub's answers, so a
    // change made to it meanwhile is kept.
    let mut said = Vec::new();
    let setup = |repo: &str| asked.get(&repo.to_lowercase()).cloned();
    if let Err(e) = change_file(&path, |d| {
        if let Some((out, set)) = lands_migrated(d, &guessed, queue_was_on, &setup) {
            *d = out;
            said = set;
        }
    }) {
        eprintln!("repos.json: can't carry landing over to lands: {e}");
        return None;
    }
    if let Err(missing) = mark_once_landed(&path, &mark, &said, queue_was_on) {
        eprintln!(
            "repos.json: lands didn't stay in the file for {}; trying again next start",
            missing.join(", ")
        );
        return None;
    }
    let mut dropped = false;
    for f in &setting_files {
        let Some(Value::Object(mut mine)) = util::read_json(f) else {
            continue;
        };
        if mine.shift_remove("pr_merge_queue").is_some() {
            dropped = true;
            let _ = util::write_atomic(f, format!("{}\n", util::json_pretty(&Value::Object(mine))));
        }
    }
    let _ = std::fs::remove_file(&guess_file);
    if said.is_empty() {
        return dropped.then(|| {
            "Each repository now lands one way, chosen in Settings' Repositories, which replaces the pr_merge_queue setting. No repository had a way to carry over, so each lands nothing until you choose.".to_string()
        });
    }
    let lines: Vec<String> = said
        .iter()
        .map(|(repo, way)| format!("{repo}: {}", way.label()))
        .collect();
    Some(format!(
        "Each repository now lands one way, chosen in Settings' Repositories. From repos.json's landing and the pr_merge_queue setting, Switchboard set:\n\n{}\n\nThe file before is kept as {}.bak.",
        lines.join("\n"),
        crate::files::shown("repos.json")
    ))
}

/// The repository at `dir`: its `owner/name` on GitHub, its worktree's top,
/// and what it is known by here.
fn repository_at(dir: &Path) -> Option<(Option<String>, PathBuf, String)> {
    let top = PathBuf::from(worktree::git_at(dir, &["rev-parse", "--show-toplevel"]).ok()?);
    let repo = repo_at(dir);
    let common = crate::cmd::land_lock::common_dir(dir);
    let key = repo.clone().unwrap_or_else(|| {
        common
            .as_deref()
            .unwrap_or(&top)
            .to_string_lossy()
            .into_owned()
    });
    Some((repo, top, key))
}

/// A direct landing's steps for the repository at `dir`, by the key
/// `landing_with` reads them under (its `owner/name`, else its shared git
/// directory), else by origin's URL as its config writes it, which an
/// `insteadOf` may point elsewhere; an error when repos.json can't be read,
/// so nothing lands without what it may say. Runs git, so off the server's
/// task.
pub fn steps_at(dir: &Path) -> Result<Steps, String> {
    let (_, _, key) =
        repository_at(dir).ok_or_else(|| format!("{} isn't in a git repository", dir.display()))?;
    let data = read()
        .map_err(|e| format!("{} can't be read ({e}); fix it first", paths::tilde(file())))?;
    let mut entry = landing_entry(&data, &key);
    if entry.is_empty()
        && let Some(configured) = worktree::git_at(dir, &["config", "--get", "remote.origin.url"])
            .ok()
            .and_then(|url| repo_of_url(&url))
    {
        entry = landing_entry(&data, &configured);
    }
    Ok(steps_of(&entry))
}

/// Where the work of the repository at `dir` is kept, its `tickets` (one
/// of `tickets::Kept`'s keys), by the key `landing_with` reads its entry under, else by
/// origin's URL as its config writes it; `None` when it isn't set, and an
/// error when repos.json can't be read. Runs git, so off the server's task.
pub fn tickets_at(dir: &Path) -> Result<Option<String>, String> {
    let (_, _, key) =
        repository_at(dir).ok_or_else(|| format!("{} isn't in a git repository", dir.display()))?;
    let data = read()
        .map_err(|e| format!("{} can't be read ({e}); fix it first", paths::tilde(file())))?;
    Ok([Some(key), configured_at(dir)]
        .into_iter()
        .flatten()
        .find_map(|k| tickets_of(&entry_of(&data, &k))))
}

/// `tickets_at` from the files alone, origin read from git's config file
/// and a repos.json that can't be read taken as empty, so no process starts:
/// what M-i and autopilot read on the server's task (`tickets::kept_known`,
/// `autopilot::mode::source_of`).
pub fn tickets_known(dir: &Path) -> Option<String> {
    let data = read_safely();
    keys_known(dir).find_map(|k| tickets_of(&entry_of(&data, &k)))
}

/// The keys the entry of the repository at `dir` may be under, from git's
/// files alone: origin's `owner/name`, then its shared git directory, as
/// `repository_at` keys one with no origin.
pub fn keys_known(dir: &Path) -> impl Iterator<Item = String> {
    let origin = crate::sources::repo::origin_of(dir).map(|r| r.to_string());
    let common = crate::tabs::git::locate(dir).map(|p| p.common_dir.to_string_lossy().into_owned());
    origin.into_iter().chain(common)
}

/// The label an issue of the repository at `dir` carries for autopilot to
/// take it, its `issueLabel`, found as `tickets_at` finds `tickets`;
/// `None` when it isn't set. Runs git, so off the server's task.
pub fn issue_label_at(dir: &Path) -> Result<Option<String>, String> {
    let (_, _, key) =
        repository_at(dir).ok_or_else(|| format!("{} isn't in a git repository", dir.display()))?;
    let data = read()
        .map_err(|e| format!("{} can't be read ({e}); fix it first", paths::tilde(file())))?;
    Ok([Some(key), configured_at(dir)]
        .into_iter()
        .flatten()
        .find_map(|k| issue_label_of(&entry_of(&data, &k))))
}

/// `issue_label_at` from the files alone, as `tickets_known` reads.
pub fn issue_label_known(dir: &Path) -> Option<String> {
    let data = read_safely();
    keys_known(dir).find_map(|k| issue_label_of(&entry_of(&data, &k)))
}

/// An entry's `issueLabel`, trimmed, when it holds one GitHub's search can
/// take.
pub fn issue_label_of(entry: &Map<String, Value>) -> Option<String> {
    entry
        .get("issueLabel")
        .and_then(Value::as_str)
        .filter(|l| label_ok(l))
        .map(|l| l.trim().to_string())
}

/// Whether `label` can be an `issueLabel`: not empty, and nothing that
/// would break out of GitHub's search for it.
pub fn label_ok(label: &str) -> bool {
    let label = label.trim();
    !label.is_empty() && !label.contains(['"', '\\', '\n', '\''])
}

/// What a reader of a repository's tickets says while its `tickets` isn't
/// set, `name` its `owner/name` or what stands for it.
pub fn unset_words(name: &str) -> String {
    format!(
        "{name}'s ticket source isn't set: choose it in Settings' Tickets row, or set \"tickets\" to \"github\", \"linear\" or \"notes\" for it in {}",
        crate::files::shown("repos.json")
    )
}

/// The name a reader gives the repository at `dir` in what it says: its
/// `owner/name`, else "This repository". Reads files alone.
pub fn name_known(dir: &Path) -> String {
    crate::sources::repo::origin_of(dir)
        .map(|r| r.to_string())
        .unwrap_or_else(|| "This repository".into())
}

/// Whether `land-lock test` in the worktree holding `dir` waits on CI
/// rather than running the tests, as it decides: its repository's `testIn`
/// is `ci` and the landing's record doesn't say CI answered `none`. From
/// the files alone, as `tickets_known` reads them, so no process starts:
/// what the sidebar asks on the server's task while one runs, to tell its
/// wait on CI from a test run.
pub fn tests_in_ci_known(dir: &Path) -> bool {
    let Some(top) = dir.ancestors().find(|d| d.join(".git").exists()) else {
        return false;
    };
    let in_ci = crate::sources::repo::origin_of(top).is_some_and(|repo| {
        steps_of(&landing_entry(&read_safely(), &repo.to_string())).test_in == "ci"
    });
    let ci_none = crate::sources::landing::read_record(top)
        .is_some_and(|r| r.get("ci").and_then(Value::as_str) == Some("none"));
    in_ci && !ci_none
}

/// `owner/name` of origin's URL as the config of the repository at `dir`
/// writes it, which an `insteadOf` may point elsewhere. Runs git.
pub fn configured_at(dir: &Path) -> Option<String> {
    worktree::git_at(dir, &["config", "--get", "remote.origin.url"])
        .ok()
        .and_then(|url| repo_of_url(&url))
}

/// An entry's `tickets`, when it holds one it can.
pub fn tickets_of(entry: &Map<String, Value>) -> Option<String> {
    entry
        .get("tickets")
        .and_then(Value::as_str)
        .filter(|v| crate::tickets::Kept::parse(v).is_some())
        .map(str::to_string)
}

fn read_file(path: &Path) -> Result<Map<String, Value>, String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return if path.exists() {
            Err("it can't be read".into())
        } else {
            Ok(Map::new())
        };
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err("it isn't a JSON object".into()),
        Err(e) => Err(e.to_string()),
    }
}

// Runs a request to its end from wherever this is: a blocking thread of
// the server's runtime, or a command with none.
fn wait_for<T>(future: impl std::future::Future<Output = T>) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => handle.block_on(future),
        Err(_) => util::block_on(future),
    }
}

/// The repository's default branch, the one answer `switchboard landing
/// --json`'s `defaultBranch` and `wt` (its branch's start and its push
/// rule) both take: GitHub's for origin's repository with the github
/// setting on, else origin's `HEAD` in the clone, else `main` or `master`,
/// whichever origin has been fetched for; none when none is known. Runs git
/// and may ask GitHub, so off the server's task.
pub fn default_branch(dir: &Path) -> Option<String> {
    repo_at(dir)
        .filter(|_| settings::is_on("github"))
        .and_then(|r| default_branch_on_github(&r).ok())
        .or_else(|| crate::worktree_sweep::default_branch(dir))
}

/// The repository's default branch as GitHub has it now.
pub fn default_branch_on_github(repo: &str) -> Result<String, String> {
    let repo = github::parse_name_with_owner(repo).ok_or("not owner/name")?;
    wait_for(github::default_branch(
        &repo,
        "landing: the repository's default branch",
    ))
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_is_found_by_its_owner_and_name_in_any_case() {
        let data = json!({ "Acme/App": { "apply": [] }, "other/x": 1 });
        let data = data.as_object().unwrap();
        assert!(entry_of(data, "acme/app").contains_key("apply"));
        assert!(entry_of(data, "Acme/App").contains_key("apply"));
        assert!(entry_of(data, "other/x").is_empty(), "not an object");
        assert!(entry_of(data, "none/here").is_empty());
        assert_eq!(
            repo_of_url("git@github.com:Acme/App.git").as_deref(),
            Some("Acme/App")
        );
        assert_eq!(repo_of_url("https://gitlab.com/a/b"), None);
    }

    #[test]
    fn each_way_reads_back_from_its_key_and_says_its_five_facts() {
        for way in Way::ALL {
            assert_eq!(Way::parse(way.key()), Some(way));
            let facts = way.facts();
            assert_eq!(
                facts.map(|(k, _)| k),
                ["a tab", "done when", "it fixes", "never", "it needs"]
            );
            assert!(facts.iter().all(|(_, v)| !v.is_empty()));
        }
        assert_eq!(Way::parse("pr"), None, "the old landing isn't a way");
        assert!(!Way::Direct.through_pr());
        assert!(Way::Service.through_pr());
    }

    #[test]
    fn lands_is_read_only_when_it_names_a_way() {
        let entry = |v: Value| json!({ "lands": v }).as_object().unwrap().clone();
        assert_eq!(
            lands_of(&entry(json!("github-queue"))),
            Some(Way::GithubQueue)
        );
        assert_eq!(lands_of(&entry(json!("pr"))), None);
        assert_eq!(lands_of(&entry(json!(true))), None);
        assert_eq!(lands_of(&Map::new()), None);
        let service = json!({ "lands": "service", "serviceLabel": " merge-ready ", "serviceCheck": "mergebot" });
        assert_eq!(
            service_of(service.as_object().unwrap()),
            Some(Service {
                label: Some("merge-ready".into()),
                comment: None,
                check: "mergebot".into()
            })
        );
        let half = json!({ "serviceLabel": "merge-ready" });
        assert_eq!(service_of(half.as_object().unwrap()), None);
        let by_comment = json!({ "serviceComment": "/merge", "serviceCheck": "mergebot" });
        let by_comment = service_of(by_comment.as_object().unwrap()).unwrap();
        assert_eq!(by_comment.label, None);
        assert_eq!(by_comment.hand_over(), "the comment /merge");
        let check_alone = json!({ "serviceCheck": "mergebot" });
        assert_eq!(
            service_of(check_alone.as_object().unwrap()),
            None,
            "no hand-over"
        );
    }

    use crate::tabs::git::tests::{Scratch, git, repo};

    struct Rig {
        scratch: Scratch,
        repos: PathBuf,
        direct: PathBuf,
    }

    /// Old settings that no test of a set way should need.
    fn no_old(repo: &str, _: &str) -> Result<(bool, Option<Setup>), String> {
        panic!("{repo}'s old settings asked for")
    }

    /// GitHub's answer for Switchboard's own repository, as the computer
    /// that met the bug had it.
    fn trains_setup() -> Setup {
        Setup {
            auto_merge: true,
            requires_pr: true,
            train_ci: true,
            ..Setup::default()
        }
    }

    fn rig(name: &str) -> Option<Rig> {
        let scratch = Scratch::new(name);
        let direct = scratch.0.join("direct");
        if !repo(&direct) {
            eprintln!("skipped: no git");
            return None;
        }
        git(
            &direct,
            &["remote", "add", "origin", "git@github.com:acme/direct.git"],
        );
        Some(Rig {
            repos: scratch.0.join("repos.json"),
            scratch,
            direct,
        })
    }

    #[test]
    fn a_repository_lands_the_way_its_entry_sets_with_its_steps() {
        let Some(r) = rig("repos-entry") else {
            return;
        };
        std::fs::write(
            &r.repos,
            r#"{"acme/direct": {"lands": "direct", "test": "npm test", "squash": false}}"#,
        )
        .unwrap();
        let answer = landing_with(&r.direct, &r.repos, Some(&no_old)).unwrap();
        assert_eq!(
            (answer.lands, answer.source.as_str()),
            (Way::Direct, "repos.json")
        );
        assert_eq!(
            answer.steps,
            Steps {
                test: Some("npm test".into()),
                test_in: "here".into(),
                squash: false,
                review: "review-loop".into(),
                before_push: String::new(),
                after_rebase: String::new(),
            },
            "what the entry leaves out takes the defaults"
        );
        std::fs::write(
            &r.repos,
            r#"{"acme/direct": {"lands": "direct", "testIn": "elsewhere"}}"#,
        )
        .unwrap();
        let answer = landing_with(&r.direct, &r.repos, Some(&no_old)).unwrap();
        assert_eq!(
            answer.steps.test_in, "here",
            "a place the tests can't run is left out, so they run in the worktree"
        );
        std::fs::write(&r.repos, "{ broken").unwrap();
        let err = landing_with(&r.direct, &r.repos, Some(&no_old)).unwrap_err();
        assert!(err.contains("can't be read"), "{err}");
        let _ = &r.scratch;
    }

    #[test]
    fn with_no_lands_nothing_lands_and_the_words_name_settings_row() {
        let Some(r) = rig("repos-unset") else {
            return;
        };
        std::fs::write(&r.repos, r#"{"acme/direct": {"tickets": "github"}}"#).unwrap();
        let err = landing_with(&r.direct, &r.repos, Some(&no_old)).unwrap_err();
        assert!(
            err.contains("acme/direct lands nothing yet") && err.contains("Settings' Repositories"),
            "{err}"
        );
        std::fs::remove_file(&r.repos).unwrap();
        assert!(
            landing_with(&r.direct, &r.repos, Some(&no_old)).is_err(),
            "no file, no way"
        );
    }

    #[test]
    fn an_old_landing_put_back_after_the_mark_is_carried_over_at_the_landing() {
        let Some(r) = rig("repos-carried") else {
            return;
        };
        std::fs::write(
            &r.repos,
            r#"{"acme/direct": {"landing": "pr", "tickets": "github"}, "acme/other": {"landing": "pr"}}"#,
        )
        .unwrap();
        let old = |repo: &str, landing: &str| {
            assert_eq!((repo, landing), ("acme/direct", "pr"));
            Ok((true, Some(trains_setup())))
        };
        let answer = landing_with(&r.direct, &r.repos, Some(&old)).unwrap();
        assert_eq!(
            (answer.lands, answer.source.as_str()),
            (Way::SwitchboardQueue, "repos.json")
        );
        let saved: Value =
            serde_json::from_str(&std::fs::read_to_string(&r.repos).unwrap()).unwrap();
        assert_eq!(
            saved,
            json!({
                "acme/direct": { "tickets": "github", "lands": "switchboard-queue" },
                "acme/other": { "landing": "pr" },
            }),
            "only the repository landing is carried over"
        );
        std::fs::write(&r.repos, r#"{"acme/direct": {"landing": "pr"}}"#).unwrap();
        let offline = |_: &str, _: &str| Err("offline".to_string());
        let err = landing_with(&r.direct, &r.repos, Some(&offline)).unwrap_err();
        assert!(
            err.contains("lands nothing yet") && err.contains("offline"),
            "{err}"
        );
        assert!(
            std::fs::read_to_string(&r.repos)
                .unwrap()
                .contains("\"landing\""),
            "a failure to ask settles no way"
        );
        assert!(
            landing_with(&r.direct, &r.repos, None).is_err(),
            "a read alone carries nothing over"
        );
        assert!(
            std::fs::read_to_string(&r.repos)
                .unwrap()
                .contains("\"landing\"")
        );
        record_at(&r.direct, Way::AutoMerge).unwrap();
        let answer = landing_with(&r.direct, &r.repos, Some(&offline)).unwrap();
        assert_eq!(
            (answer.lands, answer.source.as_str()),
            (Way::AutoMerge, "branch"),
            "a branch that keeps its way finishes it, asking nothing"
        );
    }

    #[test]
    fn a_branch_finishes_the_way_it_started_landing_with() {
        let Some(r) = rig("repos-recorded") else {
            return;
        };
        git(&r.direct, &["commit", "--allow-empty", "-qm", "one"]);
        git(&r.direct, &["checkout", "-qb", "work"]);
        std::fs::write(&r.repos, r#"{"acme/direct": {"lands": "auto-merge"}}"#).unwrap();
        assert_eq!(record_at(&r.direct, Way::AutoMerge), Ok(Way::AutoMerge));
        std::fs::write(&r.repos, r#"{"acme/direct": {"lands": "github-queue"}}"#).unwrap();
        let answer = landing_with(&r.direct, &r.repos, Some(&no_old)).unwrap();
        assert_eq!(answer.lands, Way::AutoMerge, "the old way, for this branch");
        assert_eq!(answer.source, "branch");
        assert_eq!(answer.set, Some(Way::GithubQueue));
        assert_eq!(
            record_at(&r.direct, Way::GithubQueue),
            Ok(Way::AutoMerge),
            "a way kept is never written over"
        );
        git(&r.direct, &["checkout", "-qb", "next"]);
        assert_eq!(
            landing_with(&r.direct, &r.repos, Some(&no_old))
                .unwrap()
                .lands,
            Way::GithubQueue,
            "the next branch lands the new way"
        );
    }

    #[test]
    fn set_repo_keeps_the_rest_of_an_entry_and_never_writes_over_a_file_it_cant_read() {
        let scratch = Scratch::new("repos-set");
        let path = scratch.0.join("repos.json");
        set_repo_in(&path, "acme/direct", "lands", json!("direct")).unwrap();
        set_repo_in(&path, "acme/direct", "test", json!("make check")).unwrap();
        assert_eq!(
            read_file(&path).unwrap(),
            *json!({ "acme/direct": { "lands": "direct", "test": "make check" } })
                .as_object()
                .unwrap()
        );
        assert!(
            set_repo_in(&path, "acme/direct", "lands", json!("pr"))
                .unwrap_err()
                .contains("not a value for lands")
        );
        set_repo_in(&path, "acme/direct", "lands", json!("unset")).unwrap();
        assert_eq!(
            read_file(&path).unwrap(),
            *json!({ "acme/direct": { "test": "make check" } })
                .as_object()
                .unwrap(),
            "unset, the steps kept"
        );
        set_repo_in(&path, "acme/direct", "serviceLabel", json!(" merge-ready ")).unwrap();
        set_repo_in(&path, "acme/direct", "lands", json!("service")).unwrap();
        assert!(
            set_repo_in(&path, "acme/direct", "serviceLabel", json!("")).is_err(),
            "a merge service keeps its hand-over"
        );
        set_repo_in(&path, "acme/direct", "lands", json!("unset")).unwrap();
        assert_eq!(
            read_file(&path).unwrap()["acme/direct"]["serviceLabel"],
            json!("merge-ready")
        );
        set_repo_in(&path, "acme/direct", "serviceLabel", json!("")).unwrap();
        assert!(
            read_file(&path).unwrap()["acme/direct"]
                .get("serviceLabel")
                .is_none()
        );
        assert!(
            set_repo_in(&path, "acme/direct", "review", json!("maybe"))
                .unwrap_err()
                .contains("not a value")
        );
        set_repo_in(&path, "acme/direct", "testIn", json!("ci")).unwrap();
        set_repo_in(&path, "acme/direct", "afterRebase", json!("make lint")).unwrap();
        let written = read_file(&path).unwrap();
        let steps = steps_of(&landing_entry(&written, "acme/direct"));
        assert_eq!(
            (steps.test_in.as_str(), steps.after_rebase.as_str()),
            ("ci", "make lint")
        );
        set_repo_in(&path, "acme/direct", "tickets", json!("github")).unwrap();
        let written = read_file(&path).unwrap();
        assert_eq!(
            tickets_of(&entry_of(&written, "ACME/direct")).as_deref(),
            Some("github")
        );
        set_repo_in(&path, "acme/direct", "tickets", json!("unset")).unwrap();
        assert!(
            read_file(&path).unwrap()["acme/direct"]
                .get("tickets")
                .is_none()
        );
        set_repo_in(&path, "acme/direct", "issueLabel", json!("ready")).unwrap();
        assert!(
            set_repo_in(&path, "acme/direct", "issueLabel", json!("a\"b"))
                .unwrap_err()
                .contains("not a value for issueLabel")
        );
        assert!(bak_of(&path).exists(), "the version before is kept");
        std::fs::write(&path, "{ not json").unwrap();
        assert!(
            set_repo_in(&path, "acme/direct", "lands", json!("direct"))
                .unwrap_err()
                .contains("can't be read")
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
    }

    fn answer(queue: bool, auto: bool, rule: Value, workflow: &str) -> Value {
        json!({ "repository": {
            "autoMergeAllowed": auto,
            "viewerPermission": "WRITE",
            "mergeQueue": if queue { json!({"url": "u"}) } else { Value::Null },
            "defaultBranchRef": { "name": "main", "refUpdateRule": rule },
            "object": { "entries": [
                { "name": "ci.yml", "object": { "text": workflow } },
                { "name": "README.md", "object": { "text": "ci/train-" } },
            ] } } })
    }

    #[test]
    fn github_s_answer_names_the_labels_and_the_checks_on_recent_prs_and_keeps_them() {
        let none = |_: &str| Ok(Value::Array(Vec::new()));
        let mut data = answer(false, false, Value::Null, "");
        data["repository"]["labels"] =
            json!({ "nodes": [{ "name": "merge-ready" }, { "name": "bug" }] });
        let rollup = |contexts: Value| json!({ "commits": { "nodes": [{ "commit": { "statusCheckRollup": { "contexts": { "nodes": contexts } } } }] } });
        data["repository"]["pullRequests"] = json!({ "nodes": [
            rollup(json!([{ "name": "mergebot" }, { "name": "ci" }])),
            rollup(json!([{ "context": "legacy/status" }, { "name": "ci" }])),
            { "commits": { "nodes": [{ "commit": { "statusCheckRollup": null } }] } },
        ] });
        let s = setup_of(&data, 1, &none).unwrap();
        let names = s.names.clone().unwrap();
        assert_eq!(names.labels, vec!["merge-ready", "bug"]);
        assert_eq!(names.checks, vec!["mergebot", "ci", "legacy/status"]);
        assert_eq!(Setup::of(&s.json()), Some(s.clone()), "kept as it was read");
        let mut before = s.json();
        before.as_object_mut().unwrap().remove("names");
        assert_eq!(
            Setup::of(&before).unwrap().names,
            None,
            "an answer kept before it was asked"
        );
    }

    #[test]
    fn github_s_answer_says_which_ways_are_set_up_and_a_404_ruleset_is_none() {
        let none = |_: &str| Ok(Value::Array(Vec::new()));
        let open = setup_of(&answer(false, false, Value::Null, "on: push"), 1, &none).unwrap();
        assert!(open.can_push && !open.requires_pr && !open.train_ci);
        assert_eq!(open.suggests(), Some(Way::Direct));
        let mut read_only = answer(false, false, Value::Null, "");
        read_only["repository"]["viewerPermission"] = json!("READ");
        let theirs = setup_of(&read_only, 1, &none).unwrap();
        assert!(
            !theirs.can_push && theirs.requires_pr,
            "no protection, and no push for one who can only read"
        );
        let queued = setup_of(&answer(true, false, Value::Null, ""), 1, &none).unwrap();
        assert!(queued.merge_queue && queued.requires_pr);
        assert_eq!(queued.suggests(), Some(Way::GithubQueue));
        let protected = setup_of(
            &answer(
                false,
                true,
                json!({"viewerCanPush": true, "requiredApprovingReviewCount": 0}),
                "on:\n  push:\n    branches: [main, 'ci/train-*']\n",
            ),
            1,
            &none,
        )
        .unwrap();
        assert!(
            protected.requires_pr,
            "protection asking for a PR with no approvals still asks for one"
        );
        assert!(protected.train_ci && protected.auto_merge);
        assert_eq!(protected.suggests(), Some(Way::AutoMerge));
        let ruleset = |_: &str| {
            Ok(json!([{ "type": "pull_request" },
                { "type": "required_status_checks", "parameters": { "strict_required_status_checks_policy": true } }]))
        };
        let strict =
            setup_of(&answer(false, false, Value::Null, "ci/train-"), 1, &ruleset).unwrap();
        assert!(strict.requires_pr && strict.strict);
        let failing = |_: &str| -> Result<Value, String> { Err("500".into()) };
        assert!(
            setup_of(&answer(false, false, Value::Null, ""), 1, &failing).is_err(),
            "a ruleset that can't be read is a failure to find out"
        );
    }

    #[test]
    fn a_way_not_set_up_says_what_is_missing() {
        let none = |_: &str| Ok(Value::Array(Vec::new()));
        let s = setup_of(&answer(false, false, Value::Null, ""), 1, &none).unwrap();
        let empty = Map::new();
        let missing = |way| check(way, Some(&s), &empty);
        assert_eq!(missing(Way::GithubQueue).ok, Some(false));
        assert!(missing(Way::GithubQueue).words.contains("no merge queue"));
        assert!(
            missing(Way::AutoMerge)
                .words
                .contains("auto-merge isn't allowed")
        );
        assert!(missing(Way::SwitchboardQueue).words.contains("ci/train-*"));
        assert_eq!(missing(Way::Direct).ok, Some(true));
        assert_eq!(missing(Way::Service).ok, Some(false));
        let named = json!({ "serviceLabel": "merge-ready", "serviceCheck": "mergebot" });
        let named = named.as_object().unwrap();
        assert_eq!(
            check(Way::Service, None, named).ok,
            None,
            "the label and the check not looked for yet"
        );
        let kept_before = Setup {
            names: None,
            ..s.clone()
        };
        assert_eq!(
            check(Way::Service, Some(&kept_before), named).ok,
            None,
            "an answer kept from before they were asked"
        );
        let seen = |labels: &[&str], checks: &[&str]| Setup {
            names: Some(Names {
                labels: labels.iter().map(|l| l.to_string()).collect(),
                checks: checks.iter().map(|c| c.to_string()).collect(),
            }),
            ..s.clone()
        };
        let set = check(
            Way::Service,
            Some(&seen(&["Merge-Ready"], &["ci", "mergebot"])),
            named,
        );
        assert_eq!(set.ok, Some(true), "{}", set.words);
        let unlabeled = check(Way::Service, Some(&seen(&["bug"], &["mergebot"])), named);
        assert_eq!(unlabeled.ok, Some(false));
        assert!(
            unlabeled.words.contains("no merge-ready label"),
            "{}",
            unlabeled.words
        );
        let unseen = check(Way::Service, Some(&seen(&["merge-ready"], &["ci"])), named);
        assert_eq!(unseen.ok, Some(false));
        assert!(
            unseen.words.contains("mergebot isn't reported"),
            "{}",
            unseen.words
        );
        let by_comment = json!({ "serviceComment": "/merge", "serviceCheck": "mergebot" });
        assert_eq!(
            check(
                Way::Service,
                Some(&seen(&[], &["mergebot"])),
                by_comment.as_object().unwrap()
            )
            .ok,
            Some(true),
            "a comment needs no label"
        );
        assert_eq!(
            check(Way::AutoMerge, None, &empty).ok,
            None,
            "not asked yet"
        );
    }

    #[test]
    fn a_pr_landing_with_the_queue_on_and_trains_running_carries_over_to_switchboard_s_queue() {
        let data = json!({ "acme/switchboard": { "landing": "pr" } });
        let (out, set) = lands_migrated(data.as_object().unwrap(), &[], true, &|_| {
            Some(trains_setup())
        })
        .unwrap();
        assert_eq!(
            out["acme/switchboard"],
            json!({ "lands": "switchboard-queue" })
        );
        assert_eq!(
            set,
            vec![("acme/switchboard".to_string(), Way::SwitchboardQueue)]
        );
    }

    #[test]
    fn the_queue_setting_is_read_from_settings_then_the_mark_then_github_s_trains() {
        let trains = trains_setup();
        let plain = Setup::default();
        let on = json!(true);
        assert!(queue_was_on(Some(&on), None, None), "the setting itself");
        assert!(
            queue_was_on(None, Some(&json!({ "queueWasOn": true })), None),
            "the mark"
        );
        assert!(
            !queue_was_on(None, Some(&json!({ "queueWasOn": false })), Some(&trains)),
            "the mark over GitHub's answer"
        );
        assert!(
            queue_was_on(None, Some(&json!({})), Some(&trains)),
            "an earlier build's mark: the trains say the queue was on"
        );
        assert!(!queue_was_on(None, Some(&json!({})), Some(&plain)));
        assert!(
            !queue_was_on(None, None, Some(&trains)),
            "no mark, no setting: off"
        );
    }

    #[test]
    fn the_mark_is_written_only_once_the_file_read_again_has_lands() {
        let dir = Scratch::new("repos-mark");
        let (path, mark) = (dir.0.join("repos.json"), dir.0.join("state/mark"));
        let said = vec![("acme/switchboard".to_string(), Way::SwitchboardQueue)];
        std::fs::write(&path, r#"{"acme/switchboard": {"landing": "pr"}}"#).unwrap();
        assert_eq!(
            mark_once_landed(&path, &mark, &said, true),
            Err(vec!["acme/switchboard".to_string()]),
            "a file written back from an older copy"
        );
        assert!(!mark.exists());
        std::fs::write(
            &path,
            r#"{"acme/switchboard": {"lands": "switchboard-queue"}}"#,
        )
        .unwrap();
        assert_eq!(mark_once_landed(&path, &mark, &said, true), Ok(()));
        let kept: Value = serde_json::from_str(&std::fs::read_to_string(&mark).unwrap()).unwrap();
        assert_eq!(kept["queueWasOn"], true, "{kept}");
    }

    #[test]
    fn landing_and_the_queue_setting_carry_over_once_to_one_way_each() {
        let data = json!({
            "acme/direct": { "landing": "direct", "test": "make" },
            "acme/trains": { "landing": "pr" },
            "acme/plain": { "tickets": "github" },
            "acme/set": { "landing": "pr", "lands": "github-queue" },
        });
        let data = data.as_object().unwrap();
        let guessed = vec![
            ("acme/guessed".to_string(), "direct".to_string()),
            ("acme/queued".to_string(), "pr".to_string()),
        ];
        let setup = |repo: &str| {
            Some(Setup {
                train_ci: repo == "acme/trains" || repo == "acme/plain",
                merge_queue: repo == "acme/queued",
                ..Setup::default()
            })
        };
        let (out, set) = lands_migrated(data, &guessed, true, &setup).unwrap();
        assert_eq!(
            out["acme/direct"],
            json!({ "test": "make", "lands": "direct" })
        );
        assert_eq!(out["acme/trains"], json!({ "lands": "switchboard-queue" }));
        assert_eq!(out["acme/plain"]["lands"], "switchboard-queue");
        assert_eq!(
            out["acme/set"],
            json!({ "lands": "github-queue" }),
            "a way set stays"
        );
        assert_eq!(
            out["acme/guessed"],
            json!({ "lands": "direct" }),
            "the old guess counts"
        );
        assert_eq!(
            out["acme/queued"]["lands"], "github-queue",
            "GitHub's queue where it has one"
        );
        assert_eq!(set.len(), 5);
        let (off, _) = lands_migrated(data, &[], false, &setup).unwrap();
        assert_eq!(
            off["acme/trains"]["lands"], "auto-merge",
            "the queue was off"
        );
        assert_eq!(
            lands_migrated(&out, &[], true, &setup),
            None,
            "a second time changes nothing"
        );
    }
}
