//! `switchboard landing [--json] [--start] [<dir>]`: how the repository at
//! `<dir>` (the current directory by default) lands its work, its one way
//! (`lands`: `switchboard-queue`, `github-queue`, `auto-merge`, `direct` or
//! `service`, src/repos.rs's `Way`) on the first line, then, on the second,
//! the rest as JSON: `direct`, a direct landing's steps (`test`, `testIn`,
//! `squash`, `review`, `beforePush`, `afterRebase`), null for any other
//! way; `service`, a merge service's `label` and `check`, null for any
//! other way or one not named; `steps`, the steps the way goes through, in
//! order; where the way came from (`source`: `repos.json`, or `branch` for
//! the way the branch started landing with, which it finishes), why, the
//! way repos.json sets now (`set`), and the repository. `--json` prints both
//! as one object. `--start` keeps the way on the branch (`--start=<way>`
//! the one named, the way the landing read as it began) as the one it
//! started landing with (`branch.<branch>.wtnLands`), unless it keeps one
//! already, which queue-and-clean runs as a landing begins, so a change of
//! way in Settings applies from the next landing. It fails (exit 1) when
//! repos.json can't be read, and when the repository's `lands` isn't set,
//! saying so and naming Settings' row that sets it, since nothing guesses
//! how a repository lands. queue-and-clean asks it which way to land a
//! branch, and how.
//! `target` is the branch a direct landing goes onto when it isn't the
//! default branch: the one autopilot cut the branch from, which `wt
//! --target` recorded as `branch.<branch>.wtnTarget`, with the remote branch
//! it tracks (`remote` and `remoteBranch`, null when it tracks none, and
//! then the landing only moves the local branch); null for the default
//! branch. `started` is when `wt` started the branch's item, in seconds
//! (`branch.<branch>.wtnStarted`), and `took` how long from then until
//! now (`1h12m`), for queue-and-clean's Done line; both null for a branch
//! `wt` didn't start.
//! `defaultBranch` is the repository's default branch as GitHub has it
//! now, else (the github setting off, or GitHub not answering) origin's
//! `HEAD` in the clone, else `main` or `master`, whichever origin has
//! been fetched for; null when none is known. `note` is the path of the
//! note holding the item `wt` started the branch for (its ID in
//! `branch.<branch>.wtnItem`, found in the notes directory), and `item`
//! that item's first line; both null for a branch `wt` didn't start, and
//! `note` when no note has the ID. `ciBranch` is the branch a landing
//! whose tests run in CI pushes its commit to, `ci/land-<id>`, `<id>` the
//! first 12 hex digits of git's blob hash of the branch's name and a
//! newline, so the name, a note item's words, is never published.
//! `alreadyLanded` says whether the branch's commits are already on what
//! it lands on, as fetched last (the target's remote branch, else the
//! local target, else `origin/<defaultBranch>`): `HEAD` an ancestor of
//! it, or merging `HEAD` into it changing nothing, with no merge driver
//! that could have kept its side; null when that ref isn't there.
//! `issue` is the number of the GitHub issue `wt --issue` started the
//! branch for (`branch.<branch>.wtnIssue`) when it is an issue of origin's
//! repository, for queue-and-clean's `Closes #<number>` in the PR it
//! opens; null otherwise. `issueWork` is whether the branch is work on a
//! GitHub issue at all: `branch.<branch>.wtnIssue` set (whatever its
//! repository), or the branch named `issue/<number>-<slug>` (as `wtb` may
//! make one), for which queue-and-clean asks for no note, the issue being
//! the work's record.
//! The answer is src/repos.rs's. Ported from bin/landing.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::{Value, json};

use crate::cmd::worktree_from_note::tracked;
use crate::notes::{id, store};
use crate::{repos, worktree, worktree_sweep};

/// The branch checked out at `dir`.
fn branch_at(dir: &Path) -> Option<String> {
    worktree::git_at(dir, &["rev-parse", "--abbrev-ref", "HEAD"])
        .ok()
        .filter(|b| !b.is_empty() && b != "HEAD")
}

/// The landing's target, as the module's opening says.
fn target_at(dir: &Path) -> Value {
    let Some(branch) = branch_at(dir) else {
        return Value::Null;
    };
    let Some(target) = worktree::git_at(
        dir,
        &["config", "--get", &format!("branch.{branch}.wtnTarget")],
    )
    .ok()
    .filter(|t| !t.is_empty()) else {
        return Value::Null;
    };
    let (remote, remote_branch) = match tracked(dir, &target) {
        Some((r, b)) => (json!(r), json!(b)),
        None => (Value::Null, Value::Null),
    };
    json!({ "branch": target, "remote": remote, "remoteBranch": remote_branch })
}

/// When `wt` started the branch's item, as the module's opening says.
pub fn started_at(dir: &Path) -> Option<i64> {
    let branch = branch_at(dir)?;
    worktree::git_at(
        dir,
        &["config", "--get", &format!("branch.{branch}.wtnStarted")],
    )
    .ok()?
    .trim()
    .parse::<i64>()
    .ok()
    .filter(|s| *s > 0)
}

/// The issue `wt --issue` started the branch for, as the module's opening
/// says: its number, when `reference` (`<owner>/<repo>#<n>`) names an
/// issue of `origin`.
fn issue_of(
    reference: Option<&str>,
    origin: Option<&crate::integrations::github::Repo>,
) -> Option<u64> {
    let (repo, number) = crate::autopilot::issues::parse_reference(reference?)?;
    origin.is_some_and(|o| o.same(&repo)).then_some(number)
}

/// Whether `branch` is work on a GitHub issue, as the module's opening
/// says, `reference` its `wtnIssue`.
fn issue_work(branch: &str, reference: Option<&str>) -> bool {
    let named = branch.strip_prefix("issue/").is_some_and(|rest| {
        rest.split_once('-').is_some_and(|(n, slug)| {
            !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) && !slug.is_empty()
        })
    });
    named || reference.is_some()
}

/// The note and item `wt` started the branch for, as the module's opening
/// says.
fn note_and_item_at(dir: &Path) -> (Value, Value) {
    let Some((note_id, item)) = crate::tabs::title::wt_claim_at(dir) else {
        return (Value::Null, Value::Null);
    };
    let note = note_id
        .and_then(|i| id::find_by_id(&i, &store::notes_dir()))
        .map(|f| f.to_string_lossy().into_owned());
    (json!(note), json!(item))
}

/// The landing's own branch for its CI run, as the module's opening says:
/// what `git rev-parse --abbrev-ref HEAD | git hash-object --stdin | cut
/// -c1-12` prints.
pub fn ci_branch_of(branch: &str) -> String {
    use sha1::{Digest, Sha1};
    let content = format!("{branch}\n");
    let mut hash = Sha1::new();
    hash.update(format!("blob {}\0", content.len()).as_bytes());
    hash.update(content.as_bytes());
    let hex: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    format!("ci/land-{}", &hex[..12])
}

/// The ref the branch lands on, as `alreadyLanded` compares with.
fn lands_on(target: &Value, default: Option<&str>) -> Option<String> {
    let str_of = |k: &str| target.get(k).and_then(Value::as_str);
    match (str_of("branch"), str_of("remote"), str_of("remoteBranch")) {
        (Some(_), Some(remote), Some(branch)) => Some(format!("refs/remotes/{remote}/{branch}")),
        (Some(branch), _, _) => Some(format!("refs/heads/{branch}")),
        _ => default.map(|d| format!("refs/remotes/origin/{d}")),
    }
}

/// Whether the branch's work is already where it lands, as the module's
/// opening says.
fn already_landed(dir: &Path, upstream: Option<&str>) -> Value {
    match upstream {
        Some(u) if worktree::git_ok(dir, &["rev-parse", "--verify", "-q", u]) => {
            json!(worktree_sweep::committed_on(dir, u))
        }
        _ => Value::Null,
    }
}

pub fn main(args: Vec<String>) -> ExitCode {
    let as_json = args.iter().any(|a| a == "--json");
    let usage = "usage: switchboard landing [--json] [--start[=<way>]] [<dir>]";
    let start = args.iter().any(|a| a == "--start");
    // The way the landing read when it began, kept rather than what
    // Settings holds by the time it is kept.
    let start_as = match args.iter().find_map(|a| a.strip_prefix("--start=")) {
        Some(key) => match repos::Way::parse(key) {
            Some(way) => Some(way),
            None => {
                eprintln!("landing: not a way to land: {key}\n{usage}");
                return ExitCode::from(2);
            }
        },
        None => None,
    };
    if let Some(bad) = args.iter().find(|a| {
        a.starts_with('-')
            && !["--json", "--start"].contains(&a.as_str())
            && !a.starts_with("--start=")
    }) {
        eprintln!("landing: unknown option {bad}\n{usage}");
        return ExitCode::from(2);
    }
    let dir = args
        .iter()
        .find(|a| !a.starts_with('-'))
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let mut answer = match repos::landing_for(&dir) {
        Ok(answer) => answer,
        Err(e) => {
            eprintln!("landing: {e}");
            return ExitCode::from(1);
        }
    };
    if (start || start_as.is_some()) && answer.source != "branch" {
        match repos::record_at(&dir, start_as.unwrap_or(answer.lands)) {
            Ok(kept) => answer.lands = kept,
            Err(e) => {
                eprintln!("landing: couldn't keep the way on the branch: {e}");
                return ExitCode::from(1);
            }
        }
    }
    let lands = answer.lands;
    let mut map = serde_json::Map::new();
    map.insert(
        "direct".into(),
        if lands == repos::Way::Direct {
            answer.steps.json()
        } else {
            Value::Null
        },
    );
    map.insert(
        "service".into(),
        match (&answer.service, lands) {
            (Some(s), repos::Way::Service) => json!({ "label": s.label, "check": s.check }),
            _ => Value::Null,
        },
    );
    map.insert("steps".into(), json!(lands.steps()));
    map.insert("source".into(), json!(answer.source));
    map.insert("why".into(), json!(answer.why));
    map.insert("set".into(), json!(answer.set.map(repos::Way::key)));
    map.insert("repo".into(), json!(answer.repo));
    let target = target_at(&dir);
    let default = repos::default_branch(&dir);
    let upstream = lands_on(&target, default.as_deref());
    let (note, item) = note_and_item_at(&dir);
    map.insert("target".into(), target);
    map.insert("defaultBranch".into(), json!(default));
    map.insert("note".into(), note);
    map.insert("item".into(), item);
    let branch = branch_at(&dir);
    let reference = branch
        .as_deref()
        .and_then(|b| crate::cmd::worktree_from_issue::issue_of_branch(&dir, b));
    let origin = crate::sources::repo::remotes_of(&dir).origin;
    map.insert(
        "issueWork".into(),
        json!(issue_work(
            branch.as_deref().unwrap_or_default(),
            reference.as_deref()
        )),
    );
    map.insert(
        "issue".into(),
        json!(issue_of(reference.as_deref(), origin.as_ref())),
    );
    map.insert(
        "ciBranch".into(),
        json!(branch.as_deref().map(ci_branch_of)),
    );
    map.insert(
        "alreadyLanded".into(),
        already_landed(&dir, upstream.as_deref()),
    );
    let started = started_at(&dir);
    map.insert("started".into(), json!(started));
    map.insert(
        "took".into(),
        json!(started.map(|s| crate::sidebar::text::span(crate::util::now_secs() - s))),
    );
    if as_json {
        let mut all = serde_json::Map::new();
        all.insert("lands".into(), json!(lands.key()));
        all.extend(map);
        println!("{}", Value::Object(all));
    } else {
        println!("{}\n{}", lands.key(), Value::Object(map));
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ci_branch_is_git_s_blob_hash_of_the_name_and_a_newline() {
        // `printf 'note/x\n' | git hash-object --stdin`
        let out = std::process::Command::new("git")
            .args(["hash-object", "--stdin"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut c| {
                use std::io::Write;
                c.stdin.take().unwrap().write_all(b"note/x\n")?;
                c.wait_with_output()
            })
            .unwrap();
        let hash = String::from_utf8(out.stdout).unwrap();
        assert_eq!(ci_branch_of("note/x"), format!("ci/land-{}", &hash[..12]));
    }

    #[test]
    fn the_issue_is_its_number_only_when_it_is_origin_s() {
        let origin = crate::integrations::github::parse_name_with_owner("acme/widgets");
        assert_eq!(issue_of(Some("acme/widgets#12"), origin.as_ref()), Some(12));
        assert_eq!(issue_of(Some("Acme/Widgets#12"), origin.as_ref()), Some(12));
        assert_eq!(issue_of(Some("other/widgets#12"), origin.as_ref()), None);
        assert_eq!(issue_of(Some("acme/widgets#12"), None), None);
        assert_eq!(issue_of(None, origin.as_ref()), None);
    }

    #[test]
    fn issue_work_is_a_branch_with_its_issue_kept_or_named_for_one() {
        assert!(issue_work("issue/12-make-it-blue", None));
        assert!(issue_work("feature-x", Some("acme/widgets#12")));
        assert!(!issue_work("feature-x", None));
        assert!(!issue_work("issue/twelve-x", None));
        assert!(!issue_work("issue/12", None));
        assert!(!issue_work("issues/12-x", None));
    }

    #[test]
    fn it_lands_on_the_target_s_remote_branch_else_the_local_one_else_origin_s_default() {
        let remote = json!({ "branch": "next", "remote": "up", "remoteBranch": "team/next" });
        let local = json!({ "branch": "next", "remote": null, "remoteBranch": null });
        assert_eq!(
            lands_on(&remote, Some("main")).as_deref(),
            Some("refs/remotes/up/team/next")
        );
        assert_eq!(
            lands_on(&local, Some("main")).as_deref(),
            Some("refs/heads/next")
        );
        assert_eq!(
            lands_on(&Value::Null, Some("main")).as_deref(),
            Some("refs/remotes/origin/main")
        );
        assert_eq!(lands_on(&Value::Null, None), None);
    }
}
