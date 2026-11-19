//! Landed worktrees removed on their own, which nothing in the tmux version
//! did: a worktree whose work had landed stayed until you removed it, with
//! `worktree-done` (which needs a finished Linear ticket, so a `note/`
//! branch never qualified) or Reopen's menu, and they piled up, tens of
//! gigabytes of build output each.
//!
//! A repository's worktrees under its main clone's `.claude/worktrees/`
//! (where `wt`, `wtn` and autopilot make them) are swept: one is removed
//! when it is landed and left alone. Landed means nothing uncommitted or
//! untracked, and everything its branch changed already on the default
//! branch as origin last had it: its `HEAD` there (as after a rebase and a
//! push), else the branch merged into it changing nothing (`git
//! merge-tree`, as after a squash or a cherry-pick), never patches taken as
//! the same by `git cherry`, which passes over whitespace and merges. A PR
//! merged isn't taken as landed by itself, since commits made on its branch
//! after the merge are in no PR. Left alone means no pane has it open (its
//! directory, or its tab's `@tab_dir`), and git hasn't changed its index,
//! `HEAD` or `HEAD`'s log for `RESTING` (a read such as `git log` changes
//! none of them, so it isn't proof no one looks; a clean landed worktree
//! loses nothing by going), so a tab closed for a moment, or a Claude at
//! work in no tab, keeps it. One on a detached `HEAD`, locked,
//! or someone else's PR (`trust::is_untrusted`) is never swept.
//!
//! Removal is `git worktree remove` without `--force`, so anything git
//! would lose stops it, and the branch stays, so the work can be found
//! again. Every git command here reads with `GIT_OPTIONAL_LOCKS=0`, so a
//! sweep never writes a worktree's index and so never makes it look used.
//!
//! The rule is here, on plain data and git; `serve` puts it on the server.

pub mod serve;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use crate::util;

/// Where the worktree commands make worktrees, under the main clone.
pub const UNDER: &str = crate::worktree::DEFAULT_WORKTREE_DIR;

/// How long git must have been left alone in a worktree before it is swept.
pub const RESTING: Duration = Duration::from_secs(60 * 60);

/// One repository's sweep: its main clone and the directories panes
/// have open.
#[derive(Clone, Debug, Default)]
pub struct Ask {
    pub main: PathBuf,
    pub open: Vec<PathBuf>,
}

/// A worktree as `git worktree list --porcelain` gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub path: PathBuf,
    /// Its branch; none when its `HEAD` is detached.
    pub branch: Option<String>,
    pub locked: bool,
}

/// `git worktree list --porcelain`'s entries.
pub fn parse(porcelain: &str) -> Vec<Listed> {
    porcelain
        .split("\n\n")
        .filter_map(|entry| {
            let path = entry
                .lines()
                .find_map(|l| l.strip_prefix("worktree "))
                .map(PathBuf::from)?;
            let branch = entry
                .lines()
                .find_map(|l| l.strip_prefix("branch refs/heads/"))
                .map(str::to_string);
            let locked = entry
                .lines()
                .any(|l| l == "locked" || l.starts_with("locked "));
            Some(Listed {
                path,
                branch,
                locked,
            })
        })
        .collect()
}

/// The main clone of the repository `dir` is in, when that is one whose
/// git directory is its `.git` (not a bare repository's worktree).
pub fn main_clone_of(dir: &Path) -> Option<PathBuf> {
    let (_, common) = crate::sources::landing::git_of(dir)?;
    (common.file_name()? == ".git").then(|| common.parent().map(Path::to_path_buf))?
}

/// git in `dir`, never taking the index's lock to refresh it.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = util::output_of(
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_OPTIONAL_LOCKS", "0"),
    )
    .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The repository's default branch: origin's `HEAD`, else `main` or
/// `master`, whichever origin has been fetched for (as
/// `worktree::default_start` picks); none when none is known.
pub fn default_branch(main: &Path) -> Option<String> {
    let named = git(
        main,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .and_then(|r| r.trim().strip_prefix("origin/").map(str::to_string));
    named.or_else(|| {
        ["main", "master"]
            .into_iter()
            .find(|b| git(main, &["rev-parse", "--verify", "-q", &upstream_of(b)]).is_some())
            .map(str::to_string)
    })
}

/// The remote-tracking ref of origin's `branch`, written in full, so a
/// local branch or tag named `origin/<branch>` is never taken for it.
fn upstream_of(branch: &str) -> String {
    format!("refs/remotes/origin/{branch}")
}

/// The names of what a package manager or a tool makes and nothing else
/// does, which a worktree is removed holding. A directory of any name
/// marked as a cache (`CACHEDIR.TAG`, which cargo writes in `target`) goes
/// too; a name a project may give its own data (`build`, `dist`, `out`)
/// counts for nothing.
pub const TOOL_OUTPUT: &[&str] = &[
    "node_modules",
    ".next",
    ".turbo",
    ".parcel-cache",
    ".svelte-kit",
    "__pycache__",
    ".pytest_cache",
    ".DS_Store",
];

/// Whether something git ignores in the worktree at `tree` (`rel`, as
/// `git status --ignored` names it, a directory ending in `/`) can go
/// with it: a tool's output (`TOOL_OUTPUT`, a cache directory, a `.pyc`),
/// a link (as the worktree commands link `.claude/settings.local.json`
/// from the main clone), or a file the main clone holds the same. Anything
/// else (a local database, an `.env` of its own) keeps the worktree, since
/// keeping the branch keeps none of it.
pub fn disposable(main: &Path, tree: &Path, rel: &str) -> bool {
    let rel = rel.trim_end_matches('/');
    let here = tree.join(rel);
    let named = Path::new(rel)
        .file_name()
        .is_some_and(|n| TOOL_OUTPUT.iter().any(|t| n == std::ffi::OsStr::new(t)));
    let cache = here.join("CACHEDIR.TAG").is_file();
    let linked = || std::fs::symlink_metadata(&here).is_ok_and(|m| m.file_type().is_symlink());
    let elsewhere = main.join(rel);
    // The main clone's copy counts only when it isn't this file itself,
    // through a link into the worktree.
    let its_own_copy = || {
        let ours = real(&here);
        let theirs = real(&elsewhere);
        theirs != ours && !theirs.starts_with(real(tree))
    };
    named
        || cache
        || rel.ends_with(".pyc")
        || linked()
        || (its_own_copy() && same_file(&here, &elsewhere))
}

/// Whether two paths are files of the same bytes, read a piece at a time
/// once their sizes agree, so a large file is never held whole.
fn same_file(a: &Path, b: &Path) -> bool {
    use std::io::Read;
    let size = |p: &Path| {
        std::fs::metadata(p)
            .ok()
            .filter(|m| m.is_file())
            .map(|m| m.len())
    };
    if size(a).is_none() || size(a) != size(b) {
        return false;
    }
    let (Ok(mut a), Ok(mut b)) = (std::fs::File::open(a), std::fs::File::open(b)) else {
        return false;
    };
    let (mut x, mut y) = (vec![0u8; 64 * 1024], vec![0u8; 64 * 1024]);
    loop {
        let Ok(n) = a.read(&mut x) else {
            return false;
        };
        if n == 0 {
            return true;
        }
        if b.read_exact(&mut y[..n]).is_err() || x[..n] != y[..n] {
            return false;
        }
    }
}

/// Whether any tracked file is marked so git status passes over it
/// (assume-unchanged, a lowercase tag in `git ls-files -v`, or
/// skip-worktree, `S`): its changes would go unseen and be lost.
fn hides_changes(tree: &Path) -> bool {
    git(tree, &["ls-files", "-v"]).is_none_or(|out| {
        out.lines()
            .any(|l| l.starts_with(|c: char| c.is_ascii_lowercase() || c == 'S'))
    })
}

/// Whether everything the worktree at `tree` holds is on `upstream`
/// (`origin/<default>`): nothing uncommitted or untracked, no file whose
/// changes git status passes over, nothing ignored but what is
/// `disposable`, and its commits there (`HEAD` an ancestor of it), or
/// merging them into it changing nothing, byte for byte.
pub fn landed(main: &Path, tree: &Path, upstream: &str) -> bool {
    let Some(status) = git(
        tree,
        &[
            "status",
            "--porcelain",
            "--untracked-files=normal",
            "--ignored",
        ],
    ) else {
        return false;
    };
    let only_disposable = status.lines().all(|l| match l.strip_prefix("!! ") {
        Some(rel) => disposable(main, tree, rel.trim_matches('"')),
        None => l.trim().is_empty(),
    });
    only_disposable && !hides_changes(tree) && committed_on(tree, upstream)
}

/// Whether the commits of the worktree at `tree` are on `upstream`: `HEAD`
/// an ancestor of it, or merging `HEAD` into it changing nothing, byte for
/// byte (after a squash or a cherry-pick), with no merge driver that could
/// have kept `upstream`'s side. What is uncommitted counts for nothing
/// here; `landed` checks that too. `switchboard landing --json`'s
/// `alreadyLanded`.
pub fn committed_on(tree: &Path, upstream: &str) -> bool {
    if git(tree, &["merge-base", "--is-ancestor", "HEAD", upstream]).is_some() {
        return true;
    }
    if names_a_merge_driver(tree, upstream) {
        return false;
    }
    let tree_of = |rev: &str| git(tree, &["rev-parse", &format!("{rev}^{{tree}}")]);
    let merged = git(tree, &["merge-tree", "--write-tree", upstream, "HEAD"]);
    match (merged, tree_of(upstream)) {
        (Some(merged), Some(theirs)) => merged.lines().next() == Some(theirs.trim()),
        _ => false,
    }
}

/// Whether a file the branch changed has a merge driver of its own (a
/// `merge` attribute naming one, as `merge=ours` does), in the branch's
/// attributes or `upstream`'s, or any driver git's config defines, or
/// a `merge.default` naming one for every file with none: such a driver may keep upstream's side and drop
/// the branch's, so a merge changing nothing proves nothing. Unknown
/// counts as having one.
fn names_a_merge_driver(tree: &Path, upstream: &str) -> bool {
    // Any driver git is told of: a custom one may even be named for a
    // built-in (`text`), which it then stands in for.
    let configured = git(tree, &["config", "--get-regexp", r"^merge\..*\.driver$"]);
    let default = git(tree, &["config", "--get", "merge.default"]);
    if configured.is_some() || default.is_some() {
        return true;
    }
    let Some(base) = git(tree, &["merge-base", upstream, "HEAD"]) else {
        return true;
    };
    let Some(changed) = git(
        tree,
        &["diff", "--name-only", "--no-renames", base.trim(), "HEAD"],
    ) else {
        return true;
    };
    let files: Vec<&str> = changed.lines().filter(|l| !l.is_empty()).collect();
    if files.is_empty() {
        return false;
    }
    ["HEAD", upstream].iter().any(|source| {
        merge_attributes_at(tree, source, &files).is_none_or(|out| {
            out.lines().any(|l| {
                let value = l.rsplit(": ").next().unwrap_or("");
                !matches!(value, "unspecified" | "set" | "unset")
            })
        })
    })
}

/// What `git check-attr merge` says of `files` with the attributes of
/// the commit `source`, read from a temporary index of that commit
/// (`check-attr --cached`), since `check-attr --source` only came in
/// git 2.40 and macOS ships 2.39. `None` when a git command fails.
fn merge_attributes_at(tree: &Path, source: &str, files: &[&str]) -> Option<String> {
    static MADE: AtomicU64 = AtomicU64::new(0);
    let index = std::env::temp_dir().join(format!(
        "sb-sweep-index-{}-{}",
        std::process::id(),
        MADE.fetch_add(1, Ordering::Relaxed)
    ));
    let with_index = |args: &[&str]| {
        let out = util::output_of(
            Command::new("git")
                .arg("-C")
                .arg(tree)
                .args(args)
                .env("GIT_INDEX_FILE", &index)
                .env("GIT_OPTIONAL_LOCKS", "0"),
        )
        .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let answer = with_index(&["read-tree", source]).and_then(|_| {
        let mut args = vec!["check-attr", "--cached", "merge", "--"];
        args.extend(files);
        with_index(&args)
    });
    let _ = std::fs::remove_file(&index);
    answer
}

/// When git was last used in the worktree at `tree`: its index, `HEAD`
/// or `HEAD`'s log changed, whichever is latest.
pub fn last_used(tree: &Path) -> Option<SystemTime> {
    let own = crate::sources::landing::own_git_dir(tree)?;
    ["index", "HEAD", "logs/HEAD"]
        .iter()
        .filter_map(|f| std::fs::metadata(own.join(f)).ok()?.modified().ok())
        .max()
}

/// A path as the file system has it, links resolved, else as given.
fn real(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// The worktrees of `ask.main` to remove, by the rules above, at `now`.
pub fn landed_worktrees(ask: &Ask, listed: &[Listed], now: SystemTime) -> Vec<PathBuf> {
    let under = real(&ask.main.join(UNDER));
    let Some(default) = default_branch(&ask.main) else {
        return Vec::new();
    };
    let upstream = upstream_of(&default);
    listed
        .iter()
        .filter(|w| w.branch.is_some() && !w.locked && w.path.is_dir())
        .filter_map(|w| {
            let path = real(&w.path);
            let rested = last_used(&path)
                .is_none_or(|at| now.duration_since(at).is_ok_and(|d| d >= RESTING));
            let left_alone = path.starts_with(&under)
                && path != under
                && !is_open(&path, &ask.open)
                && rested
                && !crate::trust::is_untrusted(&path);
            let is_landed = left_alone && landed(&ask.main, &path, &upstream);
            is_landed.then(|| w.path.clone())
        })
        .collect()
}

/// The default branch fetched from origin into `origin/<default>`; with
/// none known, `main`, else `master`.
pub fn fetch(main: &Path) {
    let known = default_branch(main);
    let tries: Vec<&str> = match &known {
        Some(d) => vec![d.as_str()],
        None => vec!["main", "master"],
    };
    for branch in tries {
        crate::requests::count("GitHub", "git", "sweep");
        let refspec = format!("+refs/heads/{branch}:{}", upstream_of(branch));
        if git(main, &["fetch", "-q", "origin", &refspec]).is_some() {
            return;
        }
    }
}

/// The landed worktrees of one repository, found now.
pub fn find(ask: &Ask) -> Vec<PathBuf> {
    let Some(porcelain) = git(&ask.main, &["worktree", "list", "--porcelain"]) else {
        return Vec::new();
    };
    landed_worktrees(ask, &parse(&porcelain), SystemTime::now())
}

/// Whether `tree` is still a landed worktree to remove, every rule checked
/// again now: what was true as a sweep found it may not be once the
/// worktrees before it are removed.
pub fn still_landed(ask: &Ask, tree: &Path) -> bool {
    let Some(porcelain) = git(&ask.main, &["worktree", "list", "--porcelain"]) else {
        return false;
    };
    let one: Vec<Listed> = parse(&porcelain)
        .into_iter()
        .filter(|w| w.path == tree)
        .collect();
    !one.is_empty() && !landed_worktrees(ask, &one, SystemTime::now()).is_empty()
}

/// Whether a pane has `tree` open, or a directory under it.
pub fn is_open(tree: &Path, open: &[PathBuf]) -> bool {
    let tree = real(tree);
    open.iter().any(|o| real(o).starts_with(&tree))
}

/// Each of `trees` removed from the main clone with `git worktree remove`,
/// unless git refuses; gives the ones removed.
pub fn remove(main: &Path, trees: &[PathBuf]) -> Vec<PathBuf> {
    trees
        .iter()
        .filter(|tree| {
            util::output_of(
                Command::new("git")
                    .arg("-C")
                    .arg(main)
                    .args(["worktree", "remove"])
                    .arg(tree),
            )
            .is_ok_and(|o| o.status.success())
        })
        .cloned()
        .collect()
}

/// What the server says once a sweep removed something.
pub fn words(removed: &[PathBuf]) -> String {
    let names: Vec<String> = removed
        .iter()
        .map(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
        .collect();
    let n = names.len();
    format!(
        "Removed {n} landed worktree{}, kept {} branch{}: {}",
        if n == 1 { "" } else { "s" },
        if n == 1 { "its" } else { "their" },
        if n == 1 { "" } else { "es" },
        names.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Repo {
        _dir: crate::mux::socket::testing::TempDir,
        main: PathBuf,
    }

    fn run(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@e")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@e")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// A main clone of a bare origin, with one commit pushed to main.
    fn repo() -> Repo {
        let dir = crate::mux::socket::testing::TempDir::new();
        let origin = dir.path().join("origin.git");
        let main = dir.path().join("repo");
        run(
            dir.path(),
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                origin.to_str().unwrap(),
            ],
        );
        run(
            dir.path(),
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                main.to_str().unwrap(),
            ],
        );
        run(&main, &["checkout", "-q", "-b", "main"]);
        std::fs::write(main.join("a.txt"), "a\n").unwrap();
        std::fs::write(main.join(".gitignore"), "target/\n*.local\n").unwrap();
        run(&main, &["add", "a.txt", ".gitignore"]);
        run(&main, &["commit", "-q", "-m", "start"]);
        run(&main, &["push", "-q", "origin", "main"]);
        let main = main.canonicalize().unwrap();
        Repo { _dir: dir, main }
    }

    fn worktree(repo: &Repo, name: &str) -> PathBuf {
        let path = repo.main.join(UNDER).join(name);
        run(
            &repo.main,
            &["worktree", "add", "-q", "-b", name, path.to_str().unwrap()],
        );
        path
    }

    fn commit(dir: &Path, file: &str, text: &str) {
        std::fs::write(dir.join(file), text).unwrap();
        run(dir, &["add", file]);
        run(dir, &["commit", "-q", "-m", file]);
    }

    fn found(repo: &Repo, open: &[PathBuf], now: SystemTime) -> Vec<String> {
        let ask = Ask {
            main: repo.main.clone(),
            open: open.to_vec(),
        };
        let listed = parse(&run(&repo.main, &["worktree", "list", "--porcelain"]));
        let mut names: Vec<String> = landed_worktrees(&ask, &listed, now)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_clean_worktree_whose_commits_are_on_main_is_swept_and_the_rest_are_kept() {
        let repo = repo();
        // Landed by a rebase and a push, as queue-and-clean lands.
        let pushed = worktree(&repo, "pushed");
        commit(&pushed, "b.txt", "b\n");
        run(&pushed, &["push", "-q", "origin", "HEAD:main"]);
        run(&repo.main, &["fetch", "-q", "origin"]);
        // Landed as one squashed commit of the same change.
        let squashed = worktree(&repo, "squashed");
        commit(&squashed, "c.txt", "c\n");
        commit(&squashed, "c.txt", "c2\n");
        run(&repo.main, &["merge", "-q", "--ff-only", "origin/main"]);
        std::fs::write(repo.main.join("c.txt"), "c2\n").unwrap();
        run(&repo.main, &["add", "c.txt"]);
        run(&repo.main, &["commit", "-q", "-m", "squashed"]);
        run(&repo.main, &["push", "-q", "origin", "main"]);
        // Not landed: the same patch to git cherry, but for a space.
        let spaced = worktree(&repo, "spaced");
        commit(&spaced, "g.txt", "a b\n");
        commit(&repo.main, "g.txt", "ab\n");
        run(&repo.main, &["push", "-q", "origin", "main"]);
        // Not landed: only a local branch named origin/main has it.
        let shadowed = worktree(&repo, "shadowed");
        commit(&shadowed, "s.txt", "s\n");
        run(&shadowed, &["branch", "origin/main", "HEAD"]);
        // Not landed: a commit main doesn't have.
        let unpushed = worktree(&repo, "unpushed");
        commit(&unpushed, "d.txt", "d\n");
        // Not landed: a change not committed.
        let changed = worktree(&repo, "changed");
        std::fs::write(changed.join("e.txt"), "e\n").unwrap();
        // Landed, but open in a pane (in a folder under it).
        let open = worktree(&repo, "open");
        std::fs::create_dir(open.join("sub")).unwrap();
        // Landed, but detached on purpose.
        let detached = repo.main.join(UNDER).join("detached");
        run(
            &repo.main,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                detached.to_str().unwrap(),
            ],
        );
        run(&repo.main, &["fetch", "-q", "origin"]);
        // Landed, holding a build's output and a link, which go with it.
        let built = worktree(&repo, "built");
        std::fs::create_dir_all(built.join("target/debug")).unwrap();
        std::fs::write(
            built.join("target/CACHEDIR.TAG"),
            "Signature: 8a477f597d28d172789f06886806bc55",
        )
        .unwrap();
        std::fs::write(built.join("target/debug/app"), "bin").unwrap();
        std::os::unix::fs::symlink(repo.main.join("a.txt"), built.join("linked.local")).unwrap();
        // Not landed: a file of its own git ignores.
        let local = worktree(&repo, "local");
        std::fs::write(local.join("data.local"), "rows").unwrap();
        // Not landed: the main clone's copy is a link to this one.
        let only = worktree(&repo, "only");
        std::fs::write(only.join("env.local"), "secret").unwrap();
        std::os::unix::fs::symlink(only.join("env.local"), repo.main.join("env.local")).unwrap();
        // Not landed: a folder named like a build's, with no cache mark.
        let data = worktree(&repo, "data");
        std::fs::create_dir_all(data.join("target")).unwrap();
        std::fs::write(data.join("target/rows.db"), "rows").unwrap();
        // Not landed: a change git status is told to pass over.
        let hidden = worktree(&repo, "hidden");
        run(&hidden, &["update-index", "--assume-unchanged", "a.txt"]);
        std::fs::write(hidden.join("a.txt"), "local\n").unwrap();
        let later = SystemTime::now() + RESTING + Duration::from_secs(1);
        assert_eq!(
            found(&repo, &[open.join("sub")], later),
            vec![
                "built".to_string(),
                "pushed".to_string(),
                "squashed".to_string()
            ]
        );
        // Each used just now is left alone.
        assert!(found(&repo, &[], SystemTime::now()).is_empty());
    }

    #[test]
    fn a_merge_commit_with_a_change_of_its_own_keeps_the_worktree() {
        let repo = repo();
        let evil = worktree(&repo, "evil");
        commit(&evil, "b.txt", "b\n");
        run(&evil, &["push", "-q", "origin", "HEAD:main"]);
        run(&repo.main, &["fetch", "-q", "origin"]);
        run(&repo.main, &["merge", "-q", "--ff-only", "origin/main"]);
        commit(&repo.main, "m.txt", "m\n");
        run(&repo.main, &["push", "-q", "origin", "main"]);
        run(
            &evil,
            &["merge", "-q", "--no-ff", "--no-commit", "origin/main"],
        );
        std::fs::write(evil.join("extra.txt"), "x\n").unwrap();
        run(&evil, &["add", "extra.txt"]);
        run(&evil, &["commit", "-q", "-m", "merge"]);
        let later = SystemTime::now() + RESTING + Duration::from_secs(1);
        assert!(found(&repo, &[], later).is_empty());
    }

    #[test]
    fn a_sweep_removes_the_worktree_and_keeps_its_branch() {
        let repo = repo();
        let done = worktree(&repo, "done");
        let ask = Ask {
            main: repo.main.clone(),
            ..Ask::default()
        };
        let listed = parse(&run(&repo.main, &["worktree", "list", "--porcelain"]));
        let later = SystemTime::now() + RESTING + Duration::from_secs(1);
        assert_eq!(landed_worktrees(&ask, &listed, later), vec![done.clone()]);
        // Used just now, it isn't found.
        assert!(find(&ask).is_empty());
        assert!(done.exists());
        let old = SystemTime::now() - RESTING - Duration::from_secs(60);
        let own = crate::sources::landing::own_git_dir(&done).unwrap();
        for f in ["index", "HEAD", "logs/HEAD"] {
            std::fs::File::options()
                .write(true)
                .open(own.join(f))
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
        let found = find(&ask);
        assert_eq!(found, vec![done.clone()]);
        assert_eq!(remove(&repo.main, &found), vec![done.clone()]);
        assert!(!done.exists());
        assert!(run(&repo.main, &["branch", "--list", "done"]).contains("done"));
    }

    #[test]
    fn a_file_with_a_merge_driver_of_its_own_proves_nothing_by_a_merge() {
        let repo = repo();
        let driven = worktree(&repo, "driven");
        commit(&driven, "h.txt", "ours\n");
        assert!(!names_a_merge_driver(&driven, "origin/main"));
        commit(&driven, ".gitattributes", "h.txt merge=ours\n");
        assert!(names_a_merge_driver(&driven, "origin/main"));
        commit(&driven, ".gitattributes", "h.txt -merge\n");
        assert!(!names_a_merge_driver(&driven, "origin/main"));
        run(&repo.main, &["config", "merge.text.driver", "true"]);
        assert!(names_a_merge_driver(&driven, "origin/main"));
        run(&repo.main, &["config", "--unset", "merge.text.driver"]);
        run(&repo.main, &["config", "merge.default", "ours"]);
        assert!(names_a_merge_driver(&driven, "origin/main"));
    }

    #[test]
    fn files_are_the_same_only_byte_for_byte() {
        let dir = crate::mux::socket::testing::TempDir::new();
        let file = |name: &str, text: &str| {
            let p = dir.path().join(name);
            std::fs::write(&p, text).unwrap();
            p
        };
        let a = file("a", "same");
        assert!(same_file(&a, &file("b", "same")));
        assert!(!same_file(&a, &file("c", "diff")));
        assert!(!same_file(&a, &file("d", "longer")));
        assert!(!same_file(&a, &dir.path().join("missing")));
    }

    #[test]
    fn the_words_name_each_worktree_removed() {
        assert_eq!(
            words(&[PathBuf::from("/r/.claude/worktrees/a")]),
            "Removed 1 landed worktree, kept its branch: a"
        );
        assert_eq!(
            words(&[PathBuf::from("/r/x/a"), PathBuf::from("/r/x/b")]),
            "Removed 2 landed worktrees, kept their branches: a, b"
        );
    }
}
