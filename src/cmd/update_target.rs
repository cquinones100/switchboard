//! `update-target <branch> [<dir>]`: brings a target branch (one autopilot
//! or a hand-started item lands on, rather than the default branch) up to
//! date with the default branch, as one more landing onto it: under the
//! branch's landing lock (src/cmd/land_lock.rs), so no landing onto it
//! rebases or pushes meanwhile, it merges origin's default branch into
//! origin's `<branch>`, runs the repository's `test` and `beforePush` from
//! `switchboard landing`'s steps (src/repos.rs) when they are set, and
//! pushes, never forced. A direct landing on the default branch opens a tab
//! running it for each branch items land on (`land-lock landed`, Settings'
//! `target_follows_default`), and M-p's "Bring a target branch up to date
//! with the default branch" runs it by hand.
//!
//! It works in a detached worktree of its own, `.claude/worktrees/update-<branch>`
//! under the main clone, so it changes no one's checkout, and the lock has
//! a worktree to name. Being no Claude session, it holds the lock as its
//! process (`Lock::for_process`), which keeps it while it runs. A conflict,
//! or tests that fail, stop it with exit 3: the lock let go, the worktree
//! left with the merge in progress (or the merged commit), the pane's phase
//! `merge conflict <files>` or `tests failed …`, which the Landing view
//! draws red. Resolved (and committed, for a fix) there, the same command
//! run again finishes the merge, or takes the commit as it is, and goes on.
//! Once pushed, it fast-forwards the main clone's local `<branch>` as
//! `update-main` does its main, and removes its worktree.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

use crate::cmd::land_lock::{self, Lock};
use crate::{repos, worktree};

const USAGE: &str = "Usage: update-target <branch> [<dir>]";

/// Why it stopped: a failure (exit 1), or work left for you in its worktree
/// (exit 3).
enum Stop {
    Fail(String),
    Left(String),
}

impl From<String> for Stop {
    fn from(why: String) -> Stop {
        Stop::Fail(why)
    }
}

impl From<&str> for Stop {
    fn from(why: &str) -> Stop {
        Stop::Fail(why.to_string())
    }
}

pub fn main(args: Vec<String>) -> ExitCode {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let (branch, dir) = match args.as_slice() {
        [branch] => (branch, None),
        [branch, dir] => (branch, Some(dir)),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(1);
        }
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let dir = worktree::normalize(&cwd.join(dir.map(String::as_str).unwrap_or(".")));
    let mut root = None;
    let done = update(branch.trim(), &dir, &mut root);
    // What it left for you keeps its red phase, and its tab its mark, so
    // a landing meanwhile opens no second tab on the same merge; anything
    // else ends it, and the next landing on the default branch opens one
    // again.
    if !matches!(done, Err(Stop::Left(_))) {
        land_lock::set_phase("");
        if let Some(root) = &root {
            let_go(root, branch.trim());
        }
    }
    match done {
        Ok(said) => {
            println!("{said}");
            ExitCode::SUCCESS
        }
        Err(Stop::Fail(why)) => {
            eprintln!("update-target: {why}");
            ExitCode::from(1)
        }
        Err(Stop::Left(why)) => {
            eprintln!("{why}");
            ExitCode::from(3)
        }
    }
}

/// The worktree's folder name for `branch`: one folder, whatever its `/`s,
/// and never another branch's (`team/next` and `team-next` apart), as the
/// lock's files are named.
fn folder_for(branch: &str) -> String {
    format!("update-{}", land_lock::file_suffix(branch))
}

/// Where the work comes from and goes: the default branch on its remote, and
/// the target by its local name (its lock's) and the remote branch it
/// tracks (`branch.<b>.remote` and `.merge`, as a landing onto it pushes
/// there), else the same name on the default branch's remote.
struct Refs {
    root: PathBuf,
    default: String,
    remote: String,
    branch: String,
    up_remote: String,
    up_branch: String,
}

impl Refs {
    /// The default branch's remote-tracking ref, `origin/main`.
    fn theirs(&self) -> String {
        format!("{}/{}", self.remote, self.default)
    }

    /// The target's remote-tracking ref, `origin/ux`.
    fn target(&self) -> String {
        format!("{}/{}", self.up_remote, self.up_branch)
    }

    /// Fetches both, each from its remote; says a target that isn't there.
    fn fetch(&self) -> Result<(), String> {
        let mut by_remote: Vec<(&str, Vec<String>)> = Vec::new();
        for (remote, branch) in [
            (self.remote.as_str(), self.default.as_str()),
            (self.up_remote.as_str(), self.up_branch.as_str()),
        ] {
            let spec = format!("+refs/heads/{branch}:refs/remotes/{remote}/{branch}");
            match by_remote.iter_mut().find(|(r, _)| *r == remote) {
                Some((_, specs)) if !specs.contains(&spec) => specs.push(spec),
                Some(_) => {}
                None => by_remote.push((remote, vec![spec])),
            }
        }
        for (remote, specs) in by_remote {
            worktree::count_fetch(&self.root, remote, "update target");
            let out = Command::new("git")
                .arg("-C")
                .arg(&self.root)
                .args(["fetch", "--quiet", remote])
                .args(&specs)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .output()
                .map_err(|e| format!("could not fetch from {remote}: {e}"))?;
            if out.status.success() {
                continue;
            }
            let why = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(if why.contains("couldn't find remote ref") {
                format!("{} isn't on {}", self.up_branch, self.up_remote)
            } else {
                format!("could not fetch from {remote}:\n{why}")
            });
        }
        Ok(())
    }
}

/// `root` is set to the main clone once it is known.
fn update(branch: &str, dir: &Path, found: &mut Option<PathBuf>) -> Result<String, Stop> {
    let outside = || format!("not in a git repository: {}", dir.display());
    let top = worktree::git_at(dir, &["rev-parse", "--show-toplevel"]).map_err(|_| outside())?;
    let root = worktree::repo_root_of(Path::new(&top)).ok_or_else(outside)?;
    let common = land_lock::common_dir(&root).ok_or_else(outside)?;
    *found = Some(root.clone());
    let remote = worktree::default_remote(&root).ok_or("the repository has no remote")?;
    let default = crate::tabs::git::default_branch(&common)
        .ok_or("the repository's default branch isn't known (git remote set-head origin -a)")?;
    if branch.is_empty() || branch == default {
        return Err(Stop::Fail(format!(
            "{branch} is the default branch; update-target brings another branch up to date with it"
        )));
    }
    let (up_remote, up_branch) = crate::cmd::worktree_from_note::tracked(&root, branch)
        .unwrap_or_else(|| (remote.clone(), branch.to_string()));
    let refs = Refs {
        root: root.clone(),
        default,
        remote,
        branch: branch.to_string(),
        up_remote,
        up_branch,
    };
    // Before the lock, so a branch that isn't there is said at once.
    refs.fetch()?;

    let place = root
        .join(worktree::DEFAULT_WORKTREE_DIR)
        .join(folder_for(branch));
    if !place.exists() {
        worktree::git_at(
            &root,
            &[
                "worktree",
                "add",
                "--quiet",
                "--detach",
                &place.to_string_lossy(),
                &refs.target(),
            ],
        )
        .map_err(|e| format!("couldn't make a worktree at {}: {e}", place.display()))?;
    }
    let place = place.canonicalize().unwrap_or(place);
    // Only a detached worktree of this repository's at that path is one a
    // run before left: any other directory there is someone's, and git run
    // in a plain one would act on the main clone.
    let real = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let ours = worktree::worktrees(&root)
        .unwrap_or_default()
        .into_iter()
        .any(|w| w.detached && real(&w.path) == place);
    if !ours {
        return Err(Stop::Fail(format!(
            "{} is there but isn't a detached worktree of this repository; move it away and run update-target again",
            place.display()
        )));
    }

    let lock = Lock::for_process(&common, place.clone(), branch);
    if let Some(pid) = lock.other_process_here().map_err(|e| e.to_string())? {
        return Err(Stop::Fail(format!(
            "another update-target run for {branch} is under way (process {pid}); let it finish"
        )));
    }
    land_lock::set_phase(&format!("waiting for {branch}'s landing lock"));
    if lock.take(false).map_err(|e| e.to_string())? != 0 {
        let poll = std::env::var("LAND_LOCK_POLL")
            .ok()
            .and_then(|p| p.trim().parse::<f64>().ok())
            .filter(|p| p.is_finite() && *p >= 0.0)
            .unwrap_or(5.0);
        if lock
            .wait(Duration::from_secs_f64(poll))
            .map_err(|e| e.to_string())?
            != 0
        {
            return Err(Stop::Fail(format!("taken out of {branch}'s landing queue")));
        }
    }
    let landed = land(&refs, &place);
    let _ = lock.release();
    let said = landed?;
    // Pushed, it holds nothing of yours: a run starts only with nothing
    // uncommitted there, and a step that changed a tracked file stopped it,
    // so what is left untracked (a test's report) is the steps' own.
    let removed = worktree::git_at(
        &root,
        &["worktree", "remove", "--force", &place.to_string_lossy()],
    );
    Ok(match removed {
        Ok(_) => said,
        Err(e) => format!(
            "{said}\nIts worktree, {}, couldn't be removed ({e}); remove it before the next run.",
            place.display()
        ),
    })
}

/// The landing itself, under the lock, in the worktree at `at`.
fn land(refs: &Refs, at: &Path) -> Result<String, Stop> {
    let (branch, default, root) = (refs.branch.as_str(), refs.default.as_str(), &refs.root);
    let git = |args: &[&str]| worktree::git_at(at, args).map_err(|e| e.to_string());
    let is_ancestor =
        |a: &str, b: &str| worktree::git_ok(at, &["merge-base", "--is-ancestor", a, b]);
    let short = |sha: &str| sha.chars().take(9).collect::<String>();
    let theirs = refs.theirs();
    let target = refs.target();
    let left = |what: &str| {
        Stop::Left(format!(
            "{what}\nIn {}, then run `switchboard update-target {branch}` again.",
            at.display()
        ))
    };
    land_lock::set_phase(&format!("merging {default}"));
    refs.fetch()?;

    let merging = worktree::git_ok(at, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]);
    if merging {
        let unmerged = git(&["diff", "--name-only", "--diff-filter=U"])?;
        if !unmerged.is_empty() {
            let files = unmerged.lines().collect::<Vec<_>>().join(" ");
            land_lock::set_phase(&format!("merge conflict {files}"));
            return Err(left(&format!(
                "The merge still has conflicts in {files}: resolve them and git add them"
            )));
        }
        // Only what was added, so a fix not staged, or a new file, isn't
        // left out of what is pushed.
        let unstaged = !worktree::git_ok(at, &["diff", "--quiet"])
            || !git(&["ls-files", "--others", "--exclude-standard"])?.is_empty();
        if unstaged {
            land_lock::set_phase("merge conflict: changes not added");
            return Err(left(
                "The worktree has changes not added: git add them (or throw them away)",
            ));
        }
        git(&["commit", "--quiet", "--no-edit"])?;
    } else {
        // Untracked files too: the tests would run with a file the push
        // leaves out.
        let changed = git(&["status", "--porcelain", "--untracked-files=all"])?;
        if !changed.is_empty() {
            land_lock::set_phase("tests failed: changes not committed");
            return Err(left(
                "The worktree has changes not committed: commit them (or throw them away)",
            ));
        }
        // A worktree with nothing of its own starts from origin's branch;
        // one holding a merge or a fix from a run before keeps it.
        if is_ancestor("HEAD", &target) {
            git(&["checkout", "--quiet", "--detach", &target])?;
        }
    }
    let before = git(&["rev-parse", &target])?;

    let steps = repos::steps_at(root)?;
    let run = |what: &str, command: &str| -> Result<(), Stop> {
        println!("{what}: {command}");
        let ran = Command::new("sh")
            .args(["-c", command])
            .current_dir(at)
            .stdin(Stdio::null())
            .status();
        match ran {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => {
                let code = status
                    .code()
                    .map_or("a signal".to_string(), |c| format!("exit {c}"));
                land_lock::set_phase(&format!("tests failed: {what} ({code})"));
                Err(left(&format!(
                    "{what} failed ({code}): fix it and commit the fix"
                )))
            }
            Err(e) => Err(Stop::Fail(format!("couldn't run {what}: {e}"))),
        }
    };
    let test = steps
        .test
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .map(str::to_string);
    let has_steps = test.is_some() || !steps.before_push.trim().is_empty();

    // Merged, then tested; once more when the default branch moved on while
    // the tests ran (a landing meanwhile), so the push carries it.
    loop {
        // A run taken up again after origin's branch moved on takes that in
        // too.
        for (from, message) in [
            (target.as_str(), format!("Merge {target} into {branch}")),
            (theirs.as_str(), format!("Merge {default} into {branch}")),
        ] {
            if is_ancestor(from, "HEAD") {
                continue;
            }
            let merged =
                worktree::git_ok(at, &["merge", "--quiet", "--no-edit", "-m", &message, from]);
            if !merged {
                let unmerged = git(&["diff", "--name-only", "--diff-filter=U"])?;
                if unmerged.is_empty() {
                    let _ = git(&["merge", "--abort"]);
                    return Err(Stop::Fail(format!("couldn't merge {from} into {branch}")));
                }
                let files = unmerged.lines().collect::<Vec<_>>().join(" ");
                land_lock::set_phase(&format!("merge conflict {files}"));
                return Err(left(&format!(
                    "Merging {default} into {branch} conflicts in {files}: resolve them, git add them"
                )));
            }
        }
        if git(&["rev-parse", "HEAD"])? == before {
            let local = follow_locally(root, branch, &before);
            return Ok(format!("{branch} has {default}'s work already.{local}"));
        }
        if !has_steps {
            break;
        }
        // The steps run the repository's code with your credentials,
        // unasked: only where the merge holds nothing but your own and
        // origin's commits, or the main clone is in trusted-dirs
        // (src/trust.rs).
        let home = crate::paths::home();
        if !crate::trust::listed(&home, root)
            && let Err(why) = crate::trust::yours_or_listed(&home, at)
        {
            return Err(Stop::Fail(format!(
                "{why}; or list the main clone, {}, there to run its steps",
                root.display()
            )));
        }
        if let Some(test) = &test {
            land_lock::set_phase("testing");
            run("the tests", test)?;
        }
        if !steps.before_push.trim().is_empty() {
            land_lock::set_phase("testing: before the push");
            run("before the push", &steps.before_push)?;
        }
        // What was tested is what is pushed: a step that changed a tracked
        // file (a formatter, a regenerated file) leaves it for you.
        if !worktree::git_ok(at, &["diff", "--quiet", "HEAD"]) {
            land_lock::set_phase("tests failed: they changed files");
            return Err(left(
                "The steps changed files git tracks: commit what should be pushed (or throw it away)",
            ));
        }
        refs.fetch()?;
        if is_ancestor(&theirs, "HEAD") && is_ancestor(&target, "HEAD") {
            break;
        }
        land_lock::set_phase(&format!("merging {default} again"));
    }
    let head = git(&["rev-parse", "HEAD"])?;

    land_lock::set_phase("pushing");
    let up = refs.up_remote.as_str();
    worktree::count_fetch(root, up, "update target");
    git(&[
        "push",
        "--quiet",
        up,
        &format!("HEAD:refs/heads/{}", refs.up_branch),
    ])
    .map_err(|e| format!("couldn't push {branch} to {target}: {e}"))?;
    let brought = git(&["rev-list", "--count", &format!("{before}..{theirs}")])?;
    let local = follow_locally(root, branch, &head);
    Ok(format!(
        "merged {default} into {branch}: {} ({brought} commits){local}",
        short(&head)
    ))
}

/// The main clone's local `branch` moved to `head`, as `update-main` moves
/// main: fast-forwarded where it is checked out with nothing uncommitted,
/// moved where it is checked out nowhere, and left with a word otherwise.
/// Nothing when there is no local branch of that name.
fn follow_locally(root: &Path, branch: &str, head: &str) -> String {
    let reference = format!("refs/heads/{branch}");
    let Ok(ours) = worktree::git_at(root, &["rev-parse", "--verify", "-q", &reference]) else {
        return String::new();
    };
    if !worktree::git_ok(root, &["merge-base", "--is-ancestor", &ours, head]) {
        return format!("; the local {branch} has commits of its own, so it was left as it is");
    }
    match worktree::holding_branch(root, branch) {
        Some(at) => {
            let clean = worktree::git_at(&at, &["status", "--porcelain", "--untracked-files=no"])
                .is_ok_and(|s| s.is_empty());
            if !clean || worktree::git_at(&at, &["merge", "--ff-only", "--quiet", head]).is_err() {
                return format!(
                    "; {branch} is checked out in {}, which has changes not committed, so it was left behind",
                    at.display()
                );
            }
        }
        None => {
            if worktree::git_at(root, &["update-ref", &reference, head, &ours]).is_err() {
                return format!("; couldn't move the local {branch}");
            }
        }
    }
    String::new()
}

// ── The tab that runs it ────────────────────────────────────────────────────

/// The tab option marking a tab that runs it: `<branch> <main clone>`, so a
/// second landing while one runs opens no second tab for the same branch.
pub const TAB_OPTION: &str = "@tab_update_target";

/// `TAB_OPTION`'s value for `branch` of the repository at `root`.
pub fn tab_mark(root: &Path, branch: &str) -> String {
    format!("{branch} {}", root.display())
}

/// The tab's name: the branch and where its work comes from.
pub fn tab_name(branch: &str, default: &str) -> String {
    format!("{branch} ← {default}")
}

/// The tab's command: this command for `branch`, its shell staying with
/// what it printed.
pub fn tab_command(branch: &str) -> Vec<String> {
    let script = format!("{} \"$@\"", worktree::self_command("update-target"));
    worktree::tab_command(&script, "update-target", &[branch.to_string()])
}

/// The branches other than `default` that items of the repository at
/// `root` land on: autopilot's target, while the mode is on in this repository,
/// and each `branch.<b>.wtnTarget` whose branch is still checked out in a
/// worktree (the item still in flight), each once, by name. Runs git.
pub fn targets_to_follow(root: &Path, default: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let mode = crate::autopilot::mode::Mode::read(&crate::paths::state_dir());
    let same = |a: &Path, b: &Path| {
        a == b
            || a.canonicalize()
                .ok()
                .is_some_and(|a| b.canonicalize().ok() == Some(a))
    };
    if mode.on && !mode.target.is_empty() && same(&mode.repo, root) {
        targets.push(mode.target.clone());
    }
    let checked_out: Vec<String> = worktree::worktrees(root)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|w| w.branch)
        .collect();
    let config = worktree::git_at(
        root,
        &["config", "--get-regexp", r"^branch\..*\.wtntarget$"],
    )
    .unwrap_or_default();
    for line in config.lines() {
        let Some((key, target)) = line.split_once(' ') else {
            continue;
        };
        let Some(branch) = key
            .strip_prefix("branch.")
            .and_then(|k| k.rsplit_once('.'))
            .map(|(b, _)| b)
        else {
            continue;
        };
        if checked_out.iter().any(|c| c == branch) {
            targets.push(target.trim().to_string());
        }
    }
    targets.retain(|t| !t.is_empty() && t != default);
    targets.sort();
    targets.dedup();
    targets
}

/// What `land-lock landed` does after a landing on the default branch,
/// with Settings' `target_follows_default` on: a tab in the background,
/// in the main clone at `root`, running this command for each branch
/// `targets_to_follow` names that no tab runs it for already. Asks the
/// server over the socket, from the pane the landing ran in.
pub fn follow_from_pane(root: &Path, default: &str) {
    use crate::mux::socket;
    let targets = targets_to_follow(root, default);
    if targets.is_empty() {
        return;
    }
    let list = socket::ask_ok(&["list"])
        .ok()
        .and_then(|l| serde_json::from_str::<serde_json::Value>(&l).ok())
        .unwrap_or_default();
    for target in targets {
        let mark = tab_mark(root, &target);
        let marked = tabs_marked(&list, &mark);
        if marked
            .iter()
            .any(|(_, commands)| live(root, &target, commands))
        {
            continue;
        }
        // A mark left on a tab whose run ended elsewhere (finished from a
        // terminal) goes with the new tab.
        for (tab, _) in &marked {
            let _ = socket::ask(&["set", "-t", tab, "-u", TAB_OPTION]);
        }
        let root_text = root.to_string_lossy();
        let mut args = vec!["new-tab", "-d", "-E", "-c", root_text.as_ref(), "--"];
        let command = tab_command(&target);
        args.extend(command.iter().map(String::as_str));
        let Ok(opened) = socket::ask_ok(&args) else {
            continue;
        };
        let Some(tab) = opened.split_whitespace().next() else {
            continue;
        };
        let _ = socket::ask(&["set", "-t", tab, TAB_OPTION, &mark]);
        let _ = socket::ask(&["set", "-t", tab, "@tab_name", &tab_name(&target, default)]);
    }
}

/// The mark taken off whichever tab runs (or ran) it for `branch` of the
/// repository at `root`, so the next landing opens one again, wherever this
/// run was started: that tab, another, or a terminal. Only from inside
/// Switchboard (`SWITCHBOARD` set), never a server found otherwise.
fn let_go(root: &Path, branch: &str) {
    use crate::mux::socket;
    if std::env::var_os("SWITCHBOARD").is_none_or(|s| s.is_empty()) {
        return;
    }
    let mark = tab_mark(root, branch);
    let Some(list) = socket::ask_ok(&["list"])
        .ok()
        .and_then(|l| serde_json::from_str::<serde_json::Value>(&l).ok())
    else {
        return;
    };
    for (tab, _) in tabs_marked(&list, &mark) {
        let _ = socket::ask(&["set", "-t", &tab, "-u", TAB_OPTION]);
    }
}

/// Whether a tab marked for `branch` of the repository at `root` still
/// stands for its run: a program other than a shell in front of one of its
/// panes (`commands`, the run itself), or the run's worktree there with
/// work left for you. A run that ended where its tab couldn't hear of it
/// (finished from a terminal) leaves neither.
pub fn live(root: &Path, branch: &str, commands: &[String]) -> bool {
    let shell = |c: &str| {
        matches!(
            c.trim_start_matches('-'),
            "zsh" | "bash" | "sh" | "fish" | "dash"
        )
    };
    commands.iter().any(|c| !shell(c))
        || root
            .join(worktree::DEFAULT_WORKTREE_DIR)
            .join(folder_for(branch))
            .exists()
}

/// The tabs whose `TAB_OPTION` is `mark`: each one's id and the programs
/// in front of its panes.
fn tabs_marked(list: &serde_json::Value, mark: &str) -> Vec<(String, Vec<String>)> {
    list.get("sessions")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|s| s.get("tabs")?.as_array())
        .flatten()
        .filter(|t| {
            t.get("options")
                .and_then(|o| o.get(TAB_OPTION))
                .and_then(|m| m.as_str())
                == Some(mark)
        })
        .filter_map(|t| {
            let id = t.get("id")?.as_str()?.to_string();
            let commands = t
                .get("panes")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|p| p.get("command")?.as_str().map(str::to_string))
                .collect();
            Some((id, commands))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_branch_is_one_folder() {
        assert_eq!(folder_for("ux-2-0"), "update-ux-2-0");
        assert_eq!(folder_for("team/next"), "update-team%2Fnext");
        assert_ne!(folder_for("team/next"), folder_for("team-next"));
    }

    #[test]
    fn the_tabs_running_it_are_found_by_their_mark() {
        let ids = serde_json::json!({ "sessions": [{ "tabs": [
            { "id": "@1", "options": { "@tab_update_target": "ux /r" } },
            { "id": "@2", "options": { "@tab_update_target": "ux /s" } },
        ] }] });
        assert_eq!(
            tabs_marked(&ids, "ux /r"),
            vec![("@1".to_string(), Vec::new())]
        );
        let root = Path::new("/no/such/clone");
        assert!(
            live(root, "ux", &["switchboard".into()]),
            "the run in front"
        );
        assert!(!live(root, "ux", &["-zsh".into()]), "ended, nothing left");
        assert_eq!(tab_mark(Path::new("/r"), "ux"), "ux /r");
    }
}
