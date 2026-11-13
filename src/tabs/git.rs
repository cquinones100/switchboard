//! Where a directory sits in git, read from git's own files rather than by
//! running git: the repository's directory (`.git`, or the one a worktree's
//! `.git` file points to), the directory it shares with its worktrees
//! (`commondir`), and the branch `HEAD` names. tmux/accent-color.sh and
//! tmux/path-facts.sh asked `git rev-parse` for each of these, three
//! processes a directory on every pane focus; this reads a few small files
//! instead, so it can run on the focus path and every tick without starting
//! anything.
//!
//! The branch is what `git rev-parse --abbrev-ref HEAD` prints, which the
//! color key was built from: the branch's name, or `HEAD` when it is
//! detached or names a branch with no commit yet (git prints `HEAD` then,
//! and fails, but the scripts kept what it printed). A repository keeping
//! its refs in a reftable has no file to read them from, and reads as one
//! on no branch.

use std::fs;
use std::path::{Path, PathBuf};

/// A directory's place in a git repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitPlace {
    /// What `git rev-parse --abbrev-ref HEAD` printed: the branch, or
    /// `HEAD` on none.
    pub branch: String,
    /// The repository's directory for this checkout (`--git-dir`), with
    /// links resolved: a worktree's own one under the main clone's
    /// `.git/worktrees/`.
    pub git_dir: PathBuf,
    /// The directory every worktree shares (`--git-common-dir`), with links
    /// resolved: the main clone's `.git`.
    pub common_dir: PathBuf,
}

impl GitPlace {
    /// A linked worktree's name (its directory under `.git/worktrees/`),
    /// or `None` in the main clone, as accent-color.sh told them apart: by
    /// whether the two directories differ.
    pub fn worktree(&self) -> Option<String> {
        if self.git_dir == self.common_dir {
            return None;
        }
        self.git_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
    }

    /// Whether `HEAD` is on a branch, which a PR or a ticket can be looked
    /// up by (window-pr.sh skipped `HEAD`).
    pub fn on_branch(&self) -> bool {
        !self.branch.is_empty() && self.branch != "HEAD"
    }
}

/// Where `dir` sits in git, or `None` outside any repository.
pub fn locate(dir: &Path) -> Option<GitPlace> {
    let git_dir = find_git_dir(dir)?;
    let git_dir = git_dir.canonicalize().ok()?;
    let common_dir = match fs::read_to_string(git_dir.join("commondir")) {
        Ok(text) => {
            let named = PathBuf::from(text.trim());
            let common = if named.is_absolute() {
                named
            } else {
                git_dir.join(named)
            };
            common.canonicalize().unwrap_or(common)
        }
        Err(_) => git_dir.clone(),
    };
    let branch = branch(&git_dir, &common_dir)?;
    Some(GitPlace {
        branch,
        git_dir,
        common_dir,
    })
}

/// The repository a directory belongs to, as the sidebar's repository
/// filter tells them apart: the main clone's directory, which every
/// worktree of it shares (the directory holding the `.git` all of them
/// name, or the shared directory itself for a bare one); a directory
/// outside git is its own.
pub fn main_clone(dir: &Path) -> PathBuf {
    match locate(dir) {
        Some(place) if place.common_dir.file_name().is_some_and(|n| n == ".git") => place
            .common_dir
            .parent()
            .map_or_else(|| place.common_dir.clone(), Path::to_path_buf),
        Some(place) => place.common_dir,
        None => dir.to_path_buf(),
    }
}

// The repository directory for `dir`: the nearest `.git` above it, a
// directory as a clone has, or a file saying `gitdir: <path>`, as a worktree
// or a submodule has, its path taken from the file's own directory.
fn find_git_dir(dir: &Path) -> Option<PathBuf> {
    for at in dir.ancestors() {
        let dot_git = at.join(".git");
        let Ok(meta) = fs::metadata(&dot_git) else {
            continue;
        };
        if meta.is_dir() {
            if dot_git.join("HEAD").is_file() {
                return Some(dot_git);
            }
            continue;
        }
        let text = fs::read_to_string(&dot_git).ok()?;
        let named = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
        let named = PathBuf::from(named);
        let path = if named.is_absolute() {
            named
        } else {
            at.join(named)
        };
        return path.join("HEAD").is_file().then_some(path);
    }
    None
}

// What `rev-parse --abbrev-ref HEAD` prints, from `HEAD` and the refs it
// names. `None` when `HEAD` can't be read, which git treats as no
// repository at all.
fn branch(git_dir: &Path, common_dir: &Path) -> Option<String> {
    let head = fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    let Some(target) = head.strip_prefix("ref:").map(str::trim) else {
        // A commit's id: detached.
        return Some("HEAD".into());
    };
    if !ref_exists(git_dir, common_dir, target) {
        return Some("HEAD".into());
    }
    let short = target
        .strip_prefix("refs/heads/")
        .or_else(|| target.strip_prefix("refs/"))
        .unwrap_or(target);
    Some(short.to_string())
}

// Whether a ref has a commit: its own file (in the worktree's directory or
// the shared one), or a line of `packed-refs`.
fn ref_exists(git_dir: &Path, common_dir: &Path, name: &str) -> bool {
    if git_dir.join(name).is_file() || common_dir.join(name).is_file() {
        return true;
    }
    fs::read_to_string(common_dir.join("packed-refs")).is_ok_and(|packed| {
        packed
            .lines()
            .filter(|l| !l.starts_with('#') && !l.starts_with('^'))
            .any(|l| l.split_once(' ').is_some_and(|(_, r)| r.trim() == name))
    })
}

/// Where a checkout's branch is, from git's files: the branch `HEAD`
/// names and the commit it is at, and the one the default branch is at
/// (origin's, else the local one; empty with none), so a commit made or
/// landed on the default branch changes it; `None` when `HEAD` can't be
/// read. With the commit its upstream is at (`ref_at`), what a count of
/// its commits is kept by (Reopen's chosen worktree).
pub fn head_commit(dir: &Path) -> Option<String> {
    let place = locate(dir)?;
    let head = fs::read_to_string(place.git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    // The default branch as the count compares against it, origin's else
    // the local one: the one origin/HEAD names, else main or master, read
    // without listing every branch, since this runs as mission control draws.
    let named = fs::read_to_string(place.common_dir.join("refs/remotes/origin/HEAD"))
        .ok()
        .and_then(|h| {
            h.trim()
                .strip_prefix("ref: refs/remotes/origin/")
                .map(str::to_string)
        });
    let candidates = match named {
        Some(d) => vec![d],
        None => vec!["main".to_string(), "master".to_string()],
    };
    let landed = candidates
        .iter()
        .map(|d| {
            [
                format!("refs/remotes/origin/{d}"),
                format!("refs/heads/{d}"),
            ]
            .iter()
            .find_map(|r| ref_commit(&place.git_dir, &place.common_dir, r))
            .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ");
    let Some(target) = head.strip_prefix("ref:").map(str::trim) else {
        return Some(format!("HEAD {head} {landed}"));
    };
    let at = ref_commit(&place.git_dir, &place.common_dir, target)?;
    Some(format!("{target} {at} {landed}"))
}

/// The commit a ref (`refs/remotes/origin/topic`) of the repository `dir`
/// names, from git's files; `None` when it has none.
pub fn ref_at(dir: &Path, name: &str) -> Option<String> {
    let place = locate(dir)?;
    ref_commit(&place.git_dir, &place.common_dir, name)
}

// The commit a ref names: its own file, else its line of `packed-refs`.
fn ref_commit(git_dir: &Path, common_dir: &Path, name: &str) -> Option<String> {
    for dir in [git_dir, common_dir] {
        if let Ok(text) = fs::read_to_string(dir.join(name)) {
            return Some(text.trim().to_string());
        }
    }
    let packed = fs::read_to_string(common_dir.join("packed-refs")).ok()?;
    packed
        .lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with('^'))
        .find_map(|l| {
            let (id, r) = l.split_once(' ')?;
            (r.trim() == name).then(|| id.to_string())
        })
}

/// The repository's local branches, by name, sorted: the files under
/// `refs/heads/` in the shared directory, and `packed-refs`' lines for them.
/// Reads files only, though a directory of many branches is read whole, so
/// it runs off the server's task.
pub fn local_branches(common_dir: &Path) -> Vec<String> {
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let name = format!("{prefix}{}", e.file_name().to_string_lossy());
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                walk(&e.path(), &format!("{name}/"), out);
            } else {
                out.push(name);
            }
        }
    }
    let mut out = Vec::new();
    walk(&common_dir.join("refs/heads"), "", &mut out);
    if let Ok(packed) = fs::read_to_string(common_dir.join("packed-refs")) {
        out.extend(
            packed
                .lines()
                .filter(|l| !l.starts_with('#') && !l.starts_with('^'))
                .filter_map(|l| l.split_once(' '))
                .filter_map(|(_, r)| r.trim().strip_prefix("refs/heads/"))
                .map(str::to_string),
        );
    }
    out.sort();
    out.dedup();
    out
}

/// The repository's default branch: what `refs/remotes/origin/HEAD` names,
/// else `main` or `master`, whichever is a local branch.
/// Whether origin has `branch`, as the clone last fetched it, from its
/// loose ref or `packed-refs`, with no git process.
pub fn on_origin(common_dir: &Path, branch: &str) -> bool {
    let name = format!("refs/remotes/origin/{branch}");
    common_dir.join(&name).is_file()
        || fs::read_to_string(common_dir.join("packed-refs")).is_ok_and(|packed| {
            packed
                .lines()
                .filter_map(|l| l.split_once(' '))
                .any(|(_, r)| r.trim() == name)
        })
}

pub fn default_branch(common_dir: &Path) -> Option<String> {
    if let Ok(head) = fs::read_to_string(common_dir.join("refs/remotes/origin/HEAD"))
        && let Some(name) = head.trim().strip_prefix("ref: refs/remotes/origin/")
    {
        return Some(name.to_string());
    }
    let local = local_branches(common_dir);
    ["main", "master"]
        .into_iter()
        .find(|b| local.iter().any(|l| l == b))
        .map(str::to_string)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::process::Command;

    /// A scratch directory under the system's temporary one, removed when
    /// dropped, its path with links resolved (macOS's /tmp is one).
    pub struct Scratch(pub PathBuf);

    impl Scratch {
        pub fn new(name: &str) -> Scratch {
            let dir = std::env::temp_dir().join(format!(
                "sb-tabs-{name}-{}-{}",
                std::process::id(),
                crate::util::now_ms()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Scratch(dir.canonicalize().unwrap())
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Runs git in `dir` with an identity of its own; false when git isn't
    /// there or fails.
    pub fn git(dir: &Path, args: &[&str]) -> bool {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    /// A repository at `dir` with one commit on `main`; false without git.
    pub fn repo(dir: &Path) -> bool {
        fs::create_dir_all(dir).unwrap();
        git(dir, &["init", "-q"])
            && git(dir, &["symbolic-ref", "HEAD", "refs/heads/main"])
            && git(dir, &["commit", "-q", "--allow-empty", "-m", "first"])
    }

    #[test]
    fn a_clone_its_branch_and_a_worktree() {
        let scratch = Scratch::new("git");
        let main = scratch.0.join("app");
        if !repo(&main) {
            eprintln!("skipped: no git");
            return;
        }
        let place = locate(&main).unwrap();
        assert_eq!(place.branch, "main");
        assert_eq!(place.git_dir, main.join(".git"));
        assert_eq!(place.common_dir, main.join(".git"));
        assert_eq!(place.worktree(), None);
        assert!(place.on_branch());

        // A subdirectory is in the same place.
        fs::create_dir_all(main.join("src/deep")).unwrap();
        assert_eq!(locate(&main.join("src/deep")), Some(place));

        assert!(git(&main, &["checkout", "-q", "-b", "eng-42-fix"]));
        assert_eq!(locate(&main).unwrap().branch, "eng-42-fix");

        let wt = scratch.0.join("app-wt");
        assert!(git(
            &main,
            &["worktree", "add", "-q", "-b", "other", wt.to_str().unwrap()]
        ));
        let place = locate(&wt).unwrap();
        assert_eq!(place.branch, "other");
        assert_eq!(place.common_dir, main.join(".git"));
        assert_eq!(place.git_dir, main.join(".git/worktrees/app-wt"));
        assert_eq!(place.worktree().as_deref(), Some("app-wt"));
        // A worktree and its main clone are one repository.
        assert_eq!(main_clone(&wt), main);
        assert_eq!(main_clone(&main.join("src/deep")), main);

        // Packed refs count as much as loose ones.
        assert!(git(&main, &["pack-refs", "--all"]));
        assert_eq!(locate(&main).unwrap().branch, "eng-42-fix");
        assert_eq!(locate(&wt).unwrap().branch, "other");

        // Its branches, loose and packed, nested ones by their whole name,
        // and the default with no origin the local main.
        assert!(git(&main, &["branch", "team/next"]));
        assert_eq!(
            local_branches(&main.join(".git")),
            ["eng-42-fix", "main", "other", "team/next"]
        );
        assert_eq!(default_branch(&main.join(".git")).as_deref(), Some("main"));
    }

    #[test]
    fn detached_unborn_and_outside() {
        let scratch = Scratch::new("head");
        let dir = scratch.0.join("r");
        fs::create_dir_all(&dir).unwrap();
        if !git(&dir, &["init", "-q"]) {
            eprintln!("skipped: no git");
            return;
        }
        // No commit yet: git prints HEAD.
        assert_eq!(locate(&dir).unwrap().branch, "HEAD");
        assert!(!locate(&dir).unwrap().on_branch());
        assert!(git(&dir, &["commit", "-q", "--allow-empty", "-m", "x"]));
        assert!(git(&dir, &["checkout", "-q", "--detach"]));
        assert_eq!(locate(&dir).unwrap().branch, "HEAD");

        let outside = scratch.0.join("plain");
        fs::create_dir_all(&outside).unwrap();
        assert_eq!(locate(&outside), None);
        assert_eq!(main_clone(&outside), outside);
    }

    #[test]
    fn origin_s_branches_are_read_loose_or_packed() {
        let dir = Scratch::new("on-origin");
        let common = &dir.0;
        fs::create_dir_all(common.join("refs/remotes/origin/team")).unwrap();
        fs::write(common.join("refs/remotes/origin/team/dev"), "abc\n").unwrap();
        fs::write(
            common.join("packed-refs"),
            "# pack-refs with: peeled\nabc refs/remotes/origin/release\nabc refs/heads/local\n",
        )
        .unwrap();
        assert!(on_origin(common, "team/dev"));
        assert!(on_origin(common, "release"));
        assert!(!on_origin(common, "local"));
        assert!(!on_origin(common, "gone"));
    }
}
