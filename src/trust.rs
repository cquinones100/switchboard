//! Whose code a directory holds, and so what of it Switchboard may run: the
//! one place that decides, for every command that would build a
//! repository's code, run its scripts, start Claude in it or list it as
//! work to reopen. Three answers, each kept here and nowhere else:
//!
//! - **Trusted directories** (`listed`): `~/.config/switchboard/trusted-dirs`,
//!   one absolute or `~/` path a line, blank lines and those starting with
//!   `#` ignored. It is yours, never committed and never written for you; a
//!   missing file trusts nothing, and a directory counts only when its real
//!   path is a listed one's exactly, so trusting `~/code` trusts nothing
//!   under it. `apply-actions` offers a repository's install or migrate
//!   script only in one listed, and `switchboard try` builds a worktree of
//!   someone else's commits only once it is.
//! - **Your own code** (`yours_or_listed`): a worktree whose `.git` leads
//!   back to it, whose commits beyond its origin's branches are on no other
//!   remote and were written and committed under your git email, and which
//!   isn't partway through bringing in other commits; else one listed.
//!   `switchboard try` asks it before building, as bin/switchboard-try's
//!   checks did (moved here from src/cmd/try_.rs).
//! - **Someone else's PR** (`is_untrusted`): a worktree `wtpr --untrusted`
//!   made, the default for a fork's PR, under `~/.cache/switchboard/
//!   pr-review/`, with `switchboard.untrusted` set in its own worktree
//!   config, which no file in the worktree can change. Its `CLAUDE.md`,
//!   `.claude/settings.json`, hooks and skills are its author's, so nothing
//!   starts Claude there (`claude-here`, `worktree-resume`,
//!   `worktree-from-branch`, M-t, the Slack relay's places), nothing lists
//!   it as work to reopen (`wt-revive`, Reopen's worktrees), `skill-path` passes
//!   over its skills, nothing builds it but `switchboard try --sealed`, and
//!   a listing in trusted-dirs doesn't make it trusted (`trusted`). Ported
//!   from lib/untrusted.js.
//!
//! `in_review_dir` reads a path's text alone (and the review folder's real
//! path), starting no process, so a list can be filtered on the server's
//! task; `is_untrusted` also asks git for the mark, for a worktree moved
//! out of the review folder.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::{paths, util};

/// The worktree config key `wtpr --untrusted` sets to `true`.
pub const MARK: &str = "switchboard.untrusted";

/// The review folder's place under the cache.
const REVIEW: &str = "pr-review";

/// git's files that say a merge, rebase, cherry-pick or revert is under way.
const MIDWAY: &[&str] = &[
    "MERGE_HEAD",
    "SQUASH_MSG",
    "CHERRY_PICK_HEAD",
    "REVERT_HEAD",
    "REBASE_HEAD",
    "rebase-merge",
    "rebase-apply",
];

// ── Someone else's PR ───────────────────────────────────────────────────────

/// Where `wtpr --untrusted` checks PRs out: `~/.cache/switchboard/pr-review`.
pub fn review_dir() -> PathBuf {
    review_dir_in(&paths::home())
}

/// The folder in the review folder that a repository's PRs are checked out
/// in, `<owner>-<name>`, anything but letters, digits, `.`, `_` and `-`
/// made a `-` and no two `-` together, since two repositories' PRs share
/// numbers.
pub fn review_folder(owner: &str, name: &str) -> String {
    let mut slug = String::new();
    for c in format!("{owner}-{name}").chars() {
        let c = if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            c
        } else {
            '-'
        };
        if !(c == '-' && slug.ends_with('-')) {
            slug.push(c);
        }
    }
    slug
}

/// The review folder under another home (the Slack relay's).
pub fn review_dir_in(home: &Path) -> PathBuf {
    home.join(".cache/switchboard").join(REVIEW)
}

/// Whether a path is in the review folder, by its text, the folder written
/// as the home has it or as its real path: git prints a worktree's path as
/// it was made. No process, so a list of worktrees is filtered anywhere.
pub fn in_review_dir(path: &Path) -> bool {
    in_review_dir_of(&paths::home(), path)
}

/// `in_review_dir` under another home.
pub fn in_review_dir_of(home: &Path, path: &Path) -> bool {
    let dir = review_dir_in(home);
    path.starts_with(&dir) || dir.canonicalize().is_ok_and(|real| path.starts_with(real))
}

/// Whether the worktree at `dir` carries the untrusted mark in its config.
pub fn marked(dir: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["config", "--type=bool", "--get", MARK])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|out| {
            out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "true"
        })
}

/// Whether `dir` holds someone else's PR: in the review folder, by its path
/// or its real path, or marked so in its worktree config wherever it is.
pub fn is_untrusted(dir: &Path) -> bool {
    is_untrusted_of(&paths::home(), dir)
}

/// `is_untrusted` under another home.
pub fn is_untrusted_of(home: &Path, dir: &Path) -> bool {
    in_review_dir_of(home, dir)
        || dir
            .canonicalize()
            .is_ok_and(|real| in_review_dir_of(home, &real))
        || marked(dir)
}

/// `is_untrusted` with no git run, for what asks on every hook
/// (src/assets.rs): the mark read from the worktree's own config file,
/// `<gitdir>/config.worktree`, where `git config --worktree` writes it.
pub fn is_untrusted_without_git(dir: &Path) -> bool {
    in_review_dir(dir)
        || dir.canonicalize().is_ok_and(|real| in_review_dir(&real))
        || marked_in_file(dir)
}

fn marked_in_file(dir: &Path) -> bool {
    let dot_git = dir.join(".git");
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else {
        let Ok(text) = std::fs::read_to_string(&dot_git) else {
            return false;
        };
        match text.lines().find_map(|l| l.strip_prefix("gitdir:")) {
            Some(gitdir) => dir.join(gitdir.trim()),
            None => return false,
        }
    };
    let Ok(config) = std::fs::read_to_string(git_dir.join("config.worktree")) else {
        return false;
    };
    let (section, key) = MARK.split_once('.').expect("section.key");
    let mut in_section = false;
    config.lines().map(str::trim).any(|line| {
        if let Some(header) = line.strip_prefix('[') {
            in_section = header
                .trim_end_matches(']')
                .trim()
                .eq_ignore_ascii_case(section);
            return false;
        }
        in_section
            && line.split_once('=').is_some_and(|(k, v)| {
                k.trim().eq_ignore_ascii_case(key)
                    && matches!(
                        v.trim().to_ascii_lowercase().as_str(),
                        "true" | "yes" | "on" | "1"
                    )
            })
    })
}

/// What a command that would start Claude in such a worktree says instead.
pub fn refusal(dir: &Path) -> String {
    format!(
        "{} holds a PR wtpr --untrusted set up, whose CLAUDE.md, settings and hooks are its author's, so Claude isn't started there.\n\
         Review it from your main clone (switchboard pr-risk, switchboard codex-review -C), and run its code only in switchboard try --sealed.",
        paths::tilde(dir)
    )
}

// ── Trusted directories ─────────────────────────────────────────────────────

fn real_dir(path: &Path) -> Option<PathBuf> {
    path.canonicalize().ok()
}

/// Whether `<real>/.config/switchboard/trusted-dirs` lists `dir` by its
/// exact real path: one a line, absolute or from `~/`, blank lines and
/// those starting with `#` ignored; a missing file trusts nothing. `real`
/// is your home.
pub fn listed(real: &Path, dir: &Path) -> bool {
    let Some(want) = real_dir(dir) else {
        return false;
    };
    let Some(text) = util::read_text(real.join(".config/switchboard/trusted-dirs")) else {
        return false;
    };
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| match l.strip_prefix("~/") {
            Some(rest) => real.join(rest),
            None if l == "~" => real.to_path_buf(),
            None => PathBuf::from(l),
        })
        .any(|p| real_dir(&p).as_ref() == Some(&want))
}

/// Whether a repository's own code may run here: listed in trusted-dirs,
/// and not someone else's PR checked out untrusted, which a listing doesn't
/// make trusted.
pub fn trusted(real: &Path, dir: &Path) -> bool {
    listed(real, dir) && !is_untrusted_of(real, dir)
}

// ── Your own code ───────────────────────────────────────────────────────────

/// A git command's output in `dir`, trimmed; `None` when it fails.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(dir).args(args);
    util::run_command(&mut command)
        .ok()
        .map(|s| s.trim_end().to_string())
}

/// Lines of a git command's output, none when it fails.
fn git_lines(dir: &Path, args: &[&str]) -> Vec<String> {
    git(dir, args)
        .map(|s| {
            s.lines()
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Whether the worktree's code is yours to build and run with your
/// credentials, as bin/switchboard-try decided; the refusal, saying how to
/// trust it, when it isn't. `real` is your home.
pub fn yours_or_listed(real: &Path, wt: &Path) -> Result<(), String> {
    let mut midway = None;
    let mut foreign = false;
    let mut own = leads_back(wt);
    if own {
        let ours = origin_branches(wt);
        let theirs: Vec<String> =
            git_lines(wt, &["for-each-ref", "--format=%(refname)", "refs/remotes"])
                .into_iter()
                .filter(|r| !ours.contains(r))
                .collect();
        let not_ours = |from: &[String]| -> HashSet<String> {
            let mut args: Vec<&str> = vec!["rev-list"];
            args.extend(from.iter().map(String::as_str));
            args.push("--not");
            args.extend(ours.iter().map(String::as_str));
            args.push("--");
            git_lines(wt, &args).into_iter().collect()
        };
        // A commit of HEAD's found on another remote's ref (a fork's, or
        // origin's PR heads) is someone else's.
        if !theirs.is_empty() && !not_ours(&["HEAD".to_string()]).is_disjoint(&not_ours(&theirs)) {
            foreign = true;
        }
        // Nor partway through bringing in other commits, whose files are in
        // the tree before any of them is in HEAD.
        for op in MIDWAY {
            // Relative to the worktree, as git gives it: --path-format is
            // newer than the git some still have.
            if let Some(path) = git(wt, &["rev-parse", "--git-path", op])
                && wt.join(path).exists()
            {
                midway = Some(*op);
                own = false;
            }
        }
        // And written and committed under your git email, since a PR
        // fetched straight to a local branch, or a fork's ref pruned since,
        // leaves no ref.
        let me = git(wt, &["config", "user.email"]).unwrap_or_default();
        let mut args = vec!["log", "--format=%ae%n%ce", "HEAD", "--not"];
        args.extend(ours.iter().map(String::as_str));
        args.push("--");
        if me.is_empty() || git_lines(wt, &args).iter().any(|e| *e != me) {
            foreign = true;
        }
        own = own && !foreign;
    }
    if own || listed(real, wt) {
        return Ok(());
    }
    let wt = wt.display();
    if let Some(op) = midway {
        return Err(format!(
            "{wt} is partway through bringing in other commits (git's {op} is there), whose files may not be yours; finish or abort it first, or list it in ~/.config/switchboard/trusted-dirs"
        ));
    }
    if foreign {
        return Err(format!(
            "{wt} holds commits that aren't yours (a fork's PR, say), whose build would run with your credentials; read them, then list it in ~/.config/switchboard/trusted-dirs to try it"
        ));
    }
    Err(format!(
        "{wt}'s .git doesn't lead back to it, so whose code it holds can't be told; list it in ~/.config/switchboard/trusted-dirs to try it"
    ))
}

/// Whether the worktree's git directory is its repository's own for exactly
/// this path: the main clone's `.git`, or the one the repository keeps for a
/// worktree here. A `.git` file can name any git directory, so sharing one
/// proves nothing by itself.
pub fn leads_back(wt: &Path) -> bool {
    let Some(want) = real_dir(wt) else {
        return false;
    };
    let dir = |args: &[&str]| git(wt, args).and_then(|p| real_dir(&wt.join(p)));
    let (Some(common), Some(own)) = (
        dir(&["rev-parse", "--git-common-dir"]),
        dir(&["rev-parse", "--absolute-git-dir"]),
    ) else {
        return false;
    };
    if own == common {
        return common.parent().and_then(real_dir) == Some(want);
    }
    if own.parent() != Some(common.join("worktrees").as_path()) {
        return false;
    }
    // Kept relative to the worktree's git directory by
    // worktree.useRelativePaths.
    let Some(back) = util::read_text(own.join("gitdir")) else {
        return false;
    };
    let back = own.join(back.trim());
    back.parent().and_then(real_dir) == Some(want)
}

/// Origin's branches: its refs but wtpr's `pr/` ones, and wherever a fetch
/// refspec of origin's maps anything but its branches (pull requests'
/// heads, forks' included).
fn origin_branches(wt: &Path) -> Vec<String> {
    let not_branches: Vec<String> = git_lines(wt, &["config", "--get-all", "remote.origin.fetch"])
        .iter()
        .filter_map(|spec| {
            let (from, to) = spec.trim_start_matches('+').split_once(':')?;
            (!from.starts_with("refs/heads/"))
                .then(|| to.split('*').next().unwrap_or("").to_string())
        })
        .filter(|p| !p.is_empty())
        .collect();
    git_lines(
        wt,
        &["for-each-ref", "--format=%(refname)", "refs/remotes/origin"],
    )
    .into_iter()
    .filter(|r| !r.starts_with("refs/remotes/origin/pr/"))
    .filter(|r| !not_branches.iter().any(|p| r.starts_with(p.as_str())))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn a_repositorys_review_folder_is_its_owner_and_name() {
        assert_eq!(review_folder("acme", "widgets"), "acme-widgets");
        assert_eq!(review_folder("a.b", "c_d"), "a.b-c_d");
        assert_eq!(review_folder("ac me", "w/x"), "ac-me-w-x");
    }

    /// A temporary folder under /tmp, removed when dropped.
    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str) -> Temp {
            let dir = std::env::temp_dir().join(format!(
                "sbtrust-{name}-{}-{}",
                std::process::id(),
                util::now_ms()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Temp(dir.canonicalize().unwrap())
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn sh(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .unwrap();
        assert!(ok.status.success(), "git {args:?}: {:?}", ok);
    }

    /// A repository of `me`'s, with one commit.
    fn clone_of(dir: &Path, me: &str) {
        write(&dir.join("README.md"), "mine\n");
        sh(dir, &["init", "-q"]);
        sh(dir, &["config", "user.email", me]);
        sh(dir, &["config", "user.name", "Me"]);
        sh(dir, &["add", "."]);
        sh(dir, &["commit", "-q", "-m", "first"]);
    }

    #[test]
    fn only_your_own_code_is_yours_unless_it_is_listed() {
        let t = Temp::new("trust");
        let real = t.0.join("real");
        fs::create_dir_all(&real).unwrap();
        let wt = t.0.join("wt");
        fs::create_dir_all(&wt).unwrap();
        clone_of(&wt, "me@example.com");
        assert_eq!(yours_or_listed(&real, &wt), Ok(()));

        // A commit someone else wrote, as a fork's PR brings.
        write(&wt.join("x"), "theirs\n");
        sh(&wt, &["add", "x"]);
        sh(
            &wt,
            &[
                "-c",
                "user.email=them@example.com",
                "commit",
                "-q",
                "-m",
                "theirs",
            ],
        );
        let why = yours_or_listed(&real, &wt).unwrap_err();
        assert!(why.contains("aren't yours"), "{why}");
        assert!(why.contains("trusted-dirs"), "{why}");

        // Listed by its path, it is trusted; a folder above it isn't enough.
        write(
            &real.join(".config/switchboard/trusted-dirs"),
            &format!("# mine\n{}\n", t.0.display()),
        );
        assert!(yours_or_listed(&real, &wt).is_err());
        assert!(!listed(&real, &wt));
        write(
            &real.join(".config/switchboard/trusted-dirs"),
            &format!("\n{}\n", wt.display()),
        );
        assert_eq!(yours_or_listed(&real, &wt), Ok(()));
        assert!(listed(&real, &wt) && trusted(&real, &wt));
    }

    #[test]
    fn a_worktree_partway_through_a_merge_is_refused() {
        let t = Temp::new("midway");
        let wt = t.0.join("wt");
        fs::create_dir_all(&wt).unwrap();
        clone_of(&wt, "me@example.com");
        write(
            &wt.join(".git/MERGE_HEAD"),
            "0000000000000000000000000000000000000000\n",
        );
        let why = yours_or_listed(&t.0, &wt).unwrap_err();
        assert!(why.contains("MERGE_HEAD"), "{why}");
    }

    #[test]
    fn a_linked_worktree_leads_back_to_its_clone() {
        let t = Temp::new("linked");
        let main = t.0.join("main");
        fs::create_dir_all(&main).unwrap();
        clone_of(&main, "me@example.com");
        sh(&main, &["worktree", "add", "-q", "-b", "side", "../side"]);
        let side = t.0.join("side");
        assert!(leads_back(&side));
        assert_eq!(yours_or_listed(&t.0, &side), Ok(()));
        // A copy of its .git file, elsewhere, leads to the other folder.
        let copy = t.0.join("copy");
        fs::create_dir_all(&copy).unwrap();
        fs::copy(side.join(".git"), copy.join(".git")).unwrap();
        assert!(!leads_back(&copy));
    }

    #[test]
    fn someone_elses_pr_is_known_by_its_folder_or_its_mark_and_a_listing_doesnt_trust_it() {
        let t = Temp::new("untrusted");
        let home = t.0.join("home");
        let main = t.0.join("main");
        fs::create_dir_all(&main).unwrap();
        clone_of(&main, "me@example.com");

        // In the review folder, by its text alone, whether or not it is there.
        let review = review_dir_in(&home).join("o-r/12345");
        assert!(in_review_dir_of(&home, &review));
        assert!(!in_review_dir_of(
            &home,
            &home.join(".cache/switchboard/pr-reviews/1")
        ));
        assert!(!in_review_dir_of(&home, &main));
        fs::create_dir_all(review.parent().unwrap()).unwrap();
        sh(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "pr-review/12345",
                review.to_str().unwrap(),
            ],
        );
        assert!(is_untrusted_of(&home, &review));

        // Elsewhere, by the mark in its own worktree config.
        let moved = t.0.join("moved");
        sh(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "theirs",
                moved.to_str().unwrap(),
            ],
        );
        assert!(!is_untrusted_of(&home, &moved));
        sh(&main, &["config", "extensions.worktreeConfig", "true"]);
        sh(&moved, &["config", "--worktree", MARK, "true"]);
        assert!(marked(&moved) && is_untrusted_of(&home, &moved));
        assert!(
            !marked(&main),
            "the main clone isn't marked by its worktree's config"
        );

        // Listed in trusted-dirs, it is still not trusted.
        write(
            &home.join(".config/switchboard/trusted-dirs"),
            &format!("{}\n", moved.display()),
        );
        assert!(listed(&home, &moved));
        assert!(!trusted(&home, &moved));
        assert!(refusal(&moved).contains("Claude isn't started there"));
    }
}
