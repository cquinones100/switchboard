//! `land-lock push [--asked]`: the
//! push that ends queue-and-clean's "Land on the default branch", and the
//! bookkeeping after it, done by code rather than by Claude following the
//! skill's steps one by one.
//!
//! With this worktree holding the landing lock (else exit 3), it fetches the
//! branch landed on and stops with exit 5 when that moved past the commit's
//! base, so the landing rebases again; and, without `--asked`, goes on only
//! when the land-without-asking gate
//! passes, read at the moment of pushing: the tab's `@land` is
//! `1`, `leak-check` finds nothing (in Switchboard's clone alone,
//! `dev::is_switchboard_clone`; elsewhere it isn't run), the landing's record holds a passing
//! run of its tests, it ran the review loop, and the review's recorded patch is the change being pushed.
//! Anything else exits 6, holding the lock, with the commit's subject, why
//! and leak-check's hits printed for Claude's `[always ask]` question.
//! Only then does it run the repository's `beforePush` (in a worktree of
//! your own commits or a trusted clone), which must leave the commit and
//! the tree as the gate checked them, and push;
//! `--asked` is the push after your yes. The plain command takes no
//! arguments, so every allow rule names it exactly
//! (`Bash(switchboard land-lock push)`) and none reaches `--asked`, however
//! it is quoted: Claude Code asks that one as it asked the `git push` it
//! replaces, and the server asks it again at PreToolUse outside a tab that
//! lands without asking (src/agent/hands_free.rs's `ask_push`). What the
//! landing says of its item when it lands without asking comes from the
//! tab: `@autopilot_item`, its plan (`@tab_plan_note`, else the one the
//! item links in `@tab_note`), else the `Decided hands-free` lines under it.
//!
//! The push is `git push <remote> HEAD:<branch>` as a child process, never
//! forced, with no option that skips or changes the repository's hooks, so
//! a pre-push hook (Switchboard's own checks the review) still decides; a
//! target that tracks no remote branch has its local branch moved instead.
//! Once pushed, in order: `@autopilot_pushed` set, `land-lock landed`, the
//! landing's `ci/land-<id>` deleted (with `testIn` `ci`), `land-lock
//! autolanded` and the Slack notice (for a push the gate let through), the
//! main clone fast-forwarded, the lock released. Any failure on the way
//! (`beforePush`, the push refused) releases the lock too; exit 5 and 6
//! keep it, since the landing goes on under it.
//!
//! What it prints on success, one item a line: `landed on <name>: <short
//! hash>`, then `before <commit>` when the main clone moved (the range
//! "Applying the change" reads), else `main clone not updated: <why>`;
//! then `ci/land-<id> not deleted: <why>` when its deletion failed.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use serde_json::Value;

use crate::cmd::land_lock::{self, Lock};
use crate::mux::socket;
use crate::worktree::git_said;
use crate::{repos, worktree};

/// The lock isn't this worktree's.
const NOT_HELD: u8 = 3;
/// The branch landed on moved past the commit's base.
const MOVED: u8 = 5;
/// The push waits on your answer.
const ASK: u8 = 6;

/// What the land-without-asking gate reads, gathered before it decides.
#[derive(Debug, Clone, Default)]
pub struct Gate {
    /// The tab's `@land` is `1`.
    pub land: bool,
    /// `leak-check` exited 0.
    pub leak_clean: bool,
    /// The landing had tests to run and ran them.
    pub tests_ran: bool,
    /// The repository's `review` is `review-loop`.
    pub review_loop: bool,
    /// The patch the review recorded clean, and the change being pushed.
    pub recorded: Option<String>,
    pub current: Option<String>,
}

/// Why the gate asks, one reason each condition that failed; empty when it
/// lets the push through.
pub fn why_ask(gate: &Gate) -> Vec<&'static str> {
    let mut why = Vec::new();
    if !gate.land {
        why.push("the tab doesn't land without asking");
    }
    if !gate.leak_clean {
        why.push("leak-check found something, or couldn't look");
    }
    if !gate.tests_ran {
        why.push("the landing ran no tests");
    }
    if !gate.review_loop {
        why.push("the repository's landing has no review loop");
    }
    let same =
        matches!((&gate.recorded, &gate.current), (Some(r), Some(c)) if !r.is_empty() && r == c);
    if !same {
        why.push("the change isn't the one the review found clean");
    }
    why
}

/// Whether the landing ran tests and they passed: the landing's record
/// (`land-lock record tests`, which each run of the tests writes) holding a
/// passing last run, whether `test` names the command or, not set, the
/// repository's CLAUDE.md does; `test` empty is none to run. A record that
/// wasn't written asks, as a failing run does.
pub fn tests_ran(test: Option<&str>, record: Option<&Value>) -> bool {
    test.is_none_or(|t| !t.trim().is_empty())
        && record
            .and_then(|r| r.pointer("/tests/ok"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

/// The landing's CI branch, `ci/land-<id>`: the branch's name hashed as
/// queue-and-clean's "The tests in CI" hashes it.
fn ci_branch(here: &Path, branch: &str) -> Option<String> {
    use std::io::Write;
    let mut child = Command::new("git")
        .arg("-C")
        .arg(here)
        .args(["hash-object", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    child
        .stdin
        .take()?
        .write_all(format!("{branch}\n").as_bytes())
        .ok()?;
    let out = child.wait_with_output().ok()?;
    let hash = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && hash.len() >= 12).then(|| format!("ci/land-{}", &hash[..12]))
}

/// Where the landing goes: the branch's name (the lock's and the success
/// line's), and the remote branch it pushes to, when there is one.
pub(crate) struct Onto {
    pub(crate) name: String,
    pub(crate) remote: Option<(String, String)>,
}

impl Onto {
    /// What the commit's base is checked against: the remote-tracking ref,
    /// or the local branch.
    pub(crate) fn base(&self) -> String {
        match &self.remote {
            Some((remote, branch)) => format!("{remote}/{branch}"),
            None => format!("refs/heads/{}", self.name),
        }
    }
}

/// Fetches the branch landed on, when it tracks a remote one, into its
/// tracking ref by name, which a remote fetching only some branches
/// wouldn't update by itself.
pub(crate) fn fetch(here: &Path, onto: &Onto) -> Result<(), String> {
    let Some((remote, branch)) = &onto.remote else {
        return Ok(());
    };
    worktree::count_fetch(here, remote, "landing");
    let spec = format!("+refs/heads/{branch}:refs/remotes/{remote}/{branch}");
    let (fetched, said) = git_said(here, &["fetch", "--quiet", remote, &spec]);
    if fetched {
        Ok(())
    } else {
        Err(format!("couldn't fetch {branch} from {remote}: {said}"))
    }
}

/// Whether a landing may run the repository's code in the worktree `here`
/// (`beforePush`, `land-lock test`'s tests), with your credentials: only
/// where it holds nothing but your own and origin's commits, or the main
/// clone (the parent of `common`, its git directory) is in trusted-dirs
/// (src/trust.rs), as update-target, and never someone else's PR checked
/// out untrusted.
pub(crate) fn may_run_its_code(common: &Path, here: &Path) -> bool {
    let home = crate::paths::home();
    !crate::trust::is_untrusted(here)
        && (common
            .parent()
            .is_some_and(|root| crate::trust::listed(&home, root))
            || crate::trust::yours_or_listed(&home, here).is_ok())
}

pub(crate) fn onto(common: &Path, here: &Path, target: Option<String>) -> Result<Onto, String> {
    match target {
        Some(name) => {
            let remote = crate::cmd::work_tab::tracked(here, &name);
            Ok(Onto { name, remote })
        }
        None => {
            let name = crate::tabs::git::default_branch(common)
                .ok_or("couldn't tell the default branch")?;
            Ok(Onto {
                remote: Some(("origin".to_string(), name.clone())),
                name,
            })
        }
    }
}

/// This program again, for the commands it already has (`leak-check`,
/// `codex-review`, `slack-relay`), run in `here`.
pub(crate) fn this_program(here: &Path, args: &[&str]) -> Option<std::process::Output> {
    let me = std::env::current_exe().ok()?;
    Command::new(me)
        .args(args)
        .current_dir(here)
        .stdin(Stdio::null())
        .output()
        .ok()
}

/// What the tab this runs in has an option set to, empty outside a pane.
pub(crate) fn tab_option(option: &str) -> String {
    if !socket::in_pane() {
        return String::new();
    }
    socket::ask_ok(&["show", option])
        .map(|v| v.trim().to_string())
        .unwrap_or_default()
}

pub(crate) fn main(
    lock: &Lock,
    common: &Path,
    here: &Path,
    target: Option<String>,
    args: &[String],
) -> ExitCode {
    let asked = match args {
        [] => false,
        [one] if one == "--asked" => true,
        _ => {
            eprintln!("usage: land-lock push [--asked]");
            return ExitCode::from(2);
        }
    };
    if !lock.held_here() {
        println!("the landing lock isn't this worktree's: take the lock again");
        return ExitCode::from(NOT_HELD);
    }
    let onto = match onto(common, here, target) {
        Ok(onto) => onto,
        Err(e) => return failed(lock, &e),
    };
    // The branch as it is now: a landing that moved it meanwhile is taken
    // in by rebasing again, under the lock.
    if let Err(e) = fetch(here, &onto) {
        return failed(lock, &e);
    }
    let base = onto.base();
    if !worktree::git_ok(here, &["merge-base", "--is-ancestor", &base, "HEAD"]) {
        println!("moved: {base} has moved past this commit's base, so rebase onto it again");
        return ExitCode::from(MOVED);
    }
    let steps = match repos::landing_for(here) {
        Ok(landing) => landing.steps,
        Err(e) => return failed(lock, &e),
    };
    let range = format!("{base}..HEAD");
    if !asked {
        // A rule of Switchboard's own repository alone (src/dev.rs): elsewhere
        // nothing is scanned and the gate passes it.
        let leak = crate::dev::is_switchboard_clone(here)
            .then(|| this_program(here, &["leak-check", &range]));
        let leak_clean = match &leak {
            Some(leak) => leak.as_ref().is_some_and(|o| o.status.success()),
            None => true,
        };
        let leak = leak.flatten();
        let hits = leak
            .as_ref()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        let recorded = this_program(here, &["codex-review", "--recorded"])
            .filter(|o| o.status.success())
            .and_then(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .split_whitespace()
                    .nth(2)
                    .map(str::to_string)
            });
        let current = this_program(here, &["codex-review", "--patch-id", &base, "HEAD"])
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
        let record = crate::sources::landing::read_record(here);
        let gate = Gate {
            land: tab_option(crate::mux::server::LAND) == "1",
            leak_clean,
            tests_ran: tests_ran(steps.test.as_deref(), record.as_ref()),
            review_loop: steps.review == "review-loop",
            recorded,
            current,
        };
        let why = why_ask(&gate);
        if !why.is_empty() {
            land_lock::set_phase("asking to land");
            let subject = worktree::git(here, &["log", "-1", "--format=%s"]).unwrap_or_default();
            println!("ask: {subject}");
            for reason in why {
                println!("why: {reason}");
            }
            for hit in hits.lines().filter(|l| !l.trim().is_empty()) {
                println!("leak: {hit}");
            }
            return ExitCode::from(ASK);
        }
    }
    // The commit the gate checked, which beforePush must leave as it is.
    let checked = worktree::git(here, &["rev-parse", "HEAD"]).unwrap_or_default();
    if !steps.before_push.trim().is_empty() {
        if !may_run_its_code(common, here) {
            return failed(
                lock,
                "the worktree holds code that isn't yours, so beforePush isn't run; trust the main clone in trusted-dirs to land it",
            );
        }
        land_lock::set_phase("pushing: before the push");
        let ran = Command::new("sh")
            .args(["-c", &steps.before_push])
            .current_dir(here)
            .stdin(Stdio::null())
            .output();
        match ran {
            Ok(out) if out.status.success() => {}
            Ok(out) => {
                let mut said = String::from_utf8_lossy(&out.stdout).into_owned();
                said.push_str(&String::from_utf8_lossy(&out.stderr));
                return failed(
                    lock,
                    &format!(
                        "beforePush ({}) failed:\n{}",
                        steps.before_push,
                        said.trim()
                    ),
                );
            }
            Err(e) => return failed(lock, &format!("couldn't run beforePush: {e}")),
        }
        // What was checked is what is pushed: a step that committed or
        // changed a tracked file leaves it for you.
        let now = worktree::git(here, &["rev-parse", "HEAD"]).unwrap_or_default();
        if now != checked || !worktree::git_ok(here, &["diff", "--quiet", "HEAD"]) {
            return failed(
                lock,
                "beforePush changed the commit or files git tracks: commit what should be pushed (or throw it away) and land it again",
            );
        }
    }

    // "Ask before landing" chosen while beforePush ran asks after all.
    if !asked && !steps.before_push.trim().is_empty() && tab_option(crate::mux::server::LAND) != "1"
    {
        land_lock::set_phase("asking to land");
        let subject = worktree::git(here, &["log", "-1", "--format=%s"]).unwrap_or_default();
        println!("ask: {subject}");
        println!("why: the tab doesn't land without asking");
        return ExitCode::from(ASK);
    }

    land_lock::set_phase("pushing");
    // Where the main clone was, read before a local target's move can
    // fast-forward it.
    let clone_before = common
        .parent()
        .and_then(|root| worktree::git(root, &["rev-parse", "HEAD"]).ok());
    let pushed = match &onto.remote {
        Some((remote, branch)) => {
            worktree::count_fetch(here, remote, "landing");
            git_said(here, &["push", remote, &format!("HEAD:{branch}")])
        }
        None => move_local(here, &onto.name),
    };
    if !pushed.0 {
        return failed(lock, &format!("the push was refused:\n{}", pushed.1));
    }
    let short = worktree::git(here, &["rev-parse", "--short", "HEAD"]).unwrap_or_default();
    let head = worktree::git(here, &["rev-parse", "HEAD"]).unwrap_or_default();
    if socket::in_pane() {
        let _ = socket::ask(&["set", crate::autopilot::PUSHED, &short]);
    }
    let _ = land_lock::landed(&[short.clone(), onto.name.clone()]);
    let mut ci_left = None;
    if let Some((remote, _)) = &onto.remote
        && steps.test_in == "ci"
        && let Ok(branch) = worktree::git(here, &["rev-parse", "--abbrev-ref", "HEAD"])
        && let Some(ci) = ci_branch(here, &branch)
    {
        // Gone already, or never pushed, is fine; anything else is said.
        worktree::count_fetch(here, remote, "landing");
        let (deleted, said) = git_said(here, &["push", "--quiet", remote, "--delete", &ci]);
        if !deleted && !said.contains("remote ref does not exist") {
            let why = said.lines().last().unwrap_or_default().to_string();
            ci_left = Some(format!("{ci} not deleted: {why}"));
        }
    }
    if !asked {
        let _ = land_lock::say_autolanded(&short, String::new(), None, None, Some(&onto.name));
    }
    let main_clone = follow(common, &onto, &head, clone_before);
    let _ = lock.release();
    println!("landed on {}: {short}", onto.name);
    println!("{main_clone}");
    if let Some(left) = ci_left {
        println!("{left}");
    }
    ExitCode::SUCCESS
}

/// A failure: said, the lock let go, exit 1.
fn failed(lock: &Lock, why: &str) -> ExitCode {
    let _ = lock.release();
    eprintln!("land-lock push: {why}");
    ExitCode::FAILURE
}

/// A target that tracks no remote branch, moved to the commit: fast-forwarded
/// where it is checked out, else pushed to the repository itself, never
/// forced.
fn move_local(here: &Path, name: &str) -> (bool, String) {
    match worktree::holding_branch(here, name) {
        Some(at) => {
            let head = worktree::git(here, &["rev-parse", "HEAD"]).unwrap_or_default();
            git_said(&at, &["merge", "--ff-only", "--quiet", &head])
        }
        None => git_said(here, &["push", ".", &format!("HEAD:refs/heads/{name}")]),
    }
}

/// The main clone brought up to the landed commit, as step 10 did: its
/// branch fast-forwarded when it has the branch landed on checked out, a
/// target's local branch moved when it doesn't. What it prints: `before
/// <commit>` when the main clone moved, else why not.
fn follow(common: &Path, onto: &Onto, head: &str, before: Option<String>) -> String {
    let Some(root) = common.parent().map(PathBuf::from) else {
        return "main clone not updated: no main clone".to_string();
    };
    let Some(before) = before else {
        return "main clone not updated: can't read where it is".to_string();
    };
    let on = worktree::git(&root, &["branch", "--show-current"]).unwrap_or_default();
    if on == onto.name {
        // A local target's move fast-forwarded it already.
        let now = worktree::git(&root, &["rev-parse", "HEAD"]).unwrap_or_default();
        if now == head && before != head {
            return format!("before {before}");
        }
        if before == head
            || worktree::git_ok(&root, &["merge-base", "--is-ancestor", head, &before])
        {
            return "main clone not updated: already there".to_string();
        }
        let (merged, said) = git_said(&root, &["merge", "--ff-only", "--quiet", head]);
        return if merged {
            format!("before {before}")
        } else {
            format!(
                "main clone not updated: can't fast-forward {} ({})",
                onto.name,
                said.lines().next().unwrap_or_default()
            )
        };
    }
    // A target checked out nowhere in the main clone: its local branch moved
    // when it only falls behind.
    if onto.remote.is_some()
        && Some(onto.name.as_str()) != crate::tabs::git::default_branch(common).as_deref()
    {
        let reference = format!("refs/heads/{}", onto.name);
        if let Ok(ours) = worktree::git(&root, &["rev-parse", "--verify", "-q", &reference])
            && worktree::holding_branch(&root, &onto.name).is_none()
            && worktree::git_ok(&root, &["merge-base", "--is-ancestor", &ours, head])
        {
            let _ = worktree::git(&root, &["update-ref", &reference, head, &ours]);
        }
    }
    format!("main clone not updated: it has {on} checked out")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn passing() -> Gate {
        Gate {
            land: true,
            leak_clean: true,
            tests_ran: true,
            review_loop: true,
            recorded: Some("abc".into()),
            current: Some("abc".into()),
        }
    }

    #[test]
    fn the_gate_passes_only_with_every_condition_met() {
        assert!(why_ask(&passing()).is_empty());
        type Break = fn(&mut Gate);
        let each: [(&str, Break); 7] = [
            ("land", |g| g.land = false),
            ("leak", |g| g.leak_clean = false),
            ("tests", |g| g.tests_ran = false),
            ("review", |g| g.review_loop = false),
            ("changed", |g| g.current = Some("def".into())),
            ("no record", |g| g.recorded = None),
            ("empty", |g| {
                g.recorded = Some(String::new());
                g.current = Some(String::new());
            }),
        ];
        for (what, fail) in each {
            let mut gate = passing();
            fail(&mut gate);
            assert_eq!(why_ask(&gate).len(), 1, "{what} alone asks");
        }
    }

    #[test]
    fn tests_ran_only_when_recorded_passing() {
        assert!(!tests_ran(Some("cargo test"), None));
        assert!(tests_ran(
            Some("cargo test"),
            Some(&json!({"tests": {"ok": true}}))
        ));
        assert!(!tests_ran(
            Some("cargo test"),
            Some(&json!({"tests": {"ok": false}}))
        ));
        assert!(!tests_ran(Some(" "), Some(&json!({"tests": {"ok": true}}))));
        assert!(tests_ran(None, Some(&json!({"tests": {"ok": true}}))));
        assert!(!tests_ran(None, Some(&json!({"tests": {"ok": false}}))));
        assert!(!tests_ran(None, None));
    }
}
