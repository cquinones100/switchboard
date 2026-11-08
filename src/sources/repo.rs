//! The GitHub repository a tab's directory belongs to, and which pull
//! request a tab leads to, as lib/sidebar/prs.js worked them out: its
//! `repoOf`, `goneWorktree`, `elsewhere` and `keyOf`.
//!
//! prs.js asked `git remote get-url origin` once per directory. Here the
//! origin is read from the repository's own `config` file, with no process,
//! since the server works it out on its tick; a remote set some way that
//! file doesn't show (an `include`, an `insteadOf`) is missed, as path-facts'
//! own reading of git's files misses a reftable.
//!
//! In a fork's clone, where `origin` is your fork and a remote named
//! `upstream` the project (the convention gh and GitHub's docs follow), your
//! PRs are opened against the project, so the `upstream` remote is read the
//! same way (`Remotes`), and the PR lookups ask about it as well as origin:
//! the PR board's and CI view's searches, and a branch's PR (by its owner's
//! fork, `PrKey::Branch`'s `from`, so another fork's branch of the same name
//! never counts). Only that one name, and only beside an origin on GitHub
//! naming another repository; what acts on the repository (landing,
//! `repos.json`, setup) stays with origin.
//!
//! A tab whose worktree has been deleted, as queue-and-clean does once its PR
//! merges, has no branch left to read a PR from, so its label loses the PR.
//! A worktree lives at `<repo>/.claude/worktrees/<branch>`, so such a tab's PR
//! is asked for by that branch, in that repository, unless the tab's name is
//! about another ticket now ("Shaping ABC-2: …"), whose work it has become.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use crate::integrations::github::{self, Repo};
use crate::tabs::git;

/// What a tab's PR is asked for by: its number, or, for a worktree that is
/// gone, its branch.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PrKey {
    Number {
        repo: Repo,
        number: u64,
    },
    /// The newest PR from `branch` in `repo`; with `from`, only one whose
    /// head is in `from`'s fork (a fork's branch, asked of the project its
    /// upstream names), so another fork's branch of the same name never
    /// counts.
    Branch {
        repo: Repo,
        branch: String,
        from: Option<String>,
    },
}

impl PrKey {
    pub fn repo(&self) -> &Repo {
        match self {
            PrKey::Number { repo, .. } | PrKey::Branch { repo, .. } => repo,
        }
    }

    /// prs.js's key, `owner/name#123` or `owner/name@branch` (or
    /// `owner/name@fork-owner:branch`), read back, only when every part is
    /// something GitHub could name, since the parts go into a query's text.
    pub fn parse(key: &str) -> Option<PrKey> {
        static KEY: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"^([\w.-]+)/([\w.-]+)(?:#(\d+)|@(?:([\w.-]+):)?([\w./-]+))$").unwrap()
        });
        let at = KEY.captures(key)?;
        let repo = Repo {
            owner: at[1].to_string(),
            name: at[2].to_string(),
        };
        match (at.get(3), at.get(5)) {
            (Some(n), _) => Some(PrKey::Number {
                repo,
                number: n.as_str().parse().ok()?,
            }),
            (_, Some(b)) => Some(PrKey::Branch {
                repo,
                branch: b.as_str().to_string(),
                from: at.get(4).map(|f| f.as_str().to_string()),
            }),
            _ => None,
        }
    }

    /// Whether its parts can go into a query's text as they are.
    pub fn safe(&self) -> bool {
        static PART: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[\w.-]+$").unwrap());
        static BRANCH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[\w./-]+$").unwrap());
        let repo = self.repo();
        PART.is_match(&repo.owner)
            && PART.is_match(&repo.name)
            && match self {
                PrKey::Number { .. } => true,
                PrKey::Branch { branch, from, .. } => {
                    BRANCH.is_match(branch) && from.as_deref().is_none_or(|f| PART.is_match(f))
                }
            }
    }
}

impl fmt::Display for PrKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PrKey::Number { repo, number } => write!(f, "{repo}#{number}"),
            PrKey::Branch {
                repo,
                branch,
                from: None,
            } => write!(f, "{repo}@{branch}"),
            PrKey::Branch {
                repo,
                branch,
                from: Some(from),
            } => write!(f, "{repo}@{from}:{branch}"),
        }
    }
}

/// A directory's GitHub repositories, as its git config names them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Remotes {
    /// The repository it belongs to: `origin`'s.
    pub origin: Option<Repo>,
    /// In a fork's clone, the project an `upstream` remote names, where the
    /// PRs from origin's branches are opened: only beside an origin on
    /// GitHub, and only when it names another repository there.
    pub upstream: Option<Repo>,
}

impl Remotes {
    /// The repositories its PRs are looked for in: origin's, then upstream's.
    pub fn all(&self) -> impl Iterator<Item = &Repo> {
        self.origin.iter().chain(&self.upstream)
    }
}

/// The GitHub repositories `dir` belongs to, from its config file: none
/// outside a repository.
pub fn remotes_of(dir: &Path) -> Remotes {
    git::locate(dir)
        .and_then(|place| fs::read_to_string(place.common_dir.join("config")).ok())
        .map(|config| remotes_in(&config))
        .unwrap_or_default()
}

/// The GitHub repository `dir` belongs to, from its `origin` remote, or
/// `None` outside a repository or for an origin elsewhere.
pub fn origin_of(dir: &Path) -> Option<Repo> {
    remotes_of(dir).origin
}

// The repositories a git config file's text names: origin's, and upstream's
// beside it when that is another repository on GitHub.
fn remotes_in(config: &str) -> Remotes {
    let named =
        |remote: &str| remote_url(config, remote).and_then(|url| github::parse_remote(&url));
    let origin = named("origin");
    let upstream = origin
        .as_ref()
        .and_then(|origin| named("upstream").filter(|upstream| !upstream.same(origin)));
    Remotes { origin, upstream }
}

// A remote's `url` from a git config file's text: the first in its
// `[remote "<name>"]` section.
fn remote_url(config: &str, remote: &str) -> Option<String> {
    let header = format!("[remote \"{remote}\"]");
    let mut inside = false;
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside =
                line.replace(char::is_whitespace, "") == header.replace(char::is_whitespace, "");
            continue;
        }
        if !inside {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case("url") {
            let value = value.trim();
            let value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .unwrap_or(value);
            return Some(value.to_string());
        }
    }
    None
}

/// The repository directory and branch of a tab sitting in a worktree under
/// `.claude/worktrees` that no longer exists.
pub fn gone_worktree(dir: &Path) -> Option<(PathBuf, String)> {
    static GONE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(.*)/\.claude/worktrees/([^/]+)$").unwrap());
    let text = dir.to_str()?;
    let at = GONE.captures(text)?;
    if dir.exists() {
        return None;
    }
    Some((PathBuf::from(&at[1]), at[2].to_string()))
}

/// Whether a tab's name is about another ticket than its branch: a tab
/// whose worktree merged and was removed, then used to shape a new ticket,
/// is that work now, and the old branch's PR isn't its own.
pub fn elsewhere(name: &str, branch: &str) -> bool {
    static IN_NAME: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\b([A-Z][A-Z0-9]*-\d+)\b").unwrap());
    static IN_BRANCH: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)^([a-z][a-z0-9]*-\d+)").unwrap());
    let named = IN_NAME.captures(name).map(|c| c[1].to_string());
    let ours = IN_BRANCH
        .captures(branch)
        .map(|c| c[1].to_ascii_uppercase());
    matches!((named, ours), (Some(n), Some(o)) if n != o)
}

/// What a tab's PR is asked for by, the first GitHub has an answer for
/// being its PR: its PR in the repository it is in (`repo`: where the tab
/// names' lookup found it, else the tab's origin), else a gone worktree's
/// branch (`branch_keys`), unless its name is about another ticket.
/// `remotes` reads a directory's repositories (the server's cache of
/// `remotes_of`).
pub fn keys_of(
    name: &str,
    path: Option<&Path>,
    pr: Option<u64>,
    repo: Option<&Repo>,
    remotes: &mut dyn FnMut(&Path) -> Remotes,
) -> Vec<PrKey> {
    if let (Some(repo), Some(number)) = (repo, pr) {
        return vec![PrKey::Number {
            repo: repo.clone(),
            number,
        }];
    }
    let Some((root, branch)) = path.and_then(gone_worktree) else {
        return Vec::new();
    };
    if elsewhere(name, &branch) {
        return Vec::new();
    }
    branch_keys(&remotes(&root), &branch)
}

/// What a branch's PR is asked for by, origin's first so its PR wins: the
/// branch in origin, and in a fork's clone the branch from origin's owner's
/// fork in the project upstream names. None without an origin on GitHub.
pub fn branch_keys(remotes: &Remotes, branch: &str) -> Vec<PrKey> {
    let Some(origin) = &remotes.origin else {
        return Vec::new();
    };
    let mut keys = vec![PrKey::Branch {
        repo: origin.clone(),
        branch: branch.to_string(),
        from: None,
    }];
    if let Some(upstream) = &remotes.upstream {
        keys.push(PrKey::Branch {
            repo: upstream.clone(),
            branch: branch.to_string(),
            from: Some(origin.owner.clone()),
        });
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabs::git::tests::{Scratch, git as run_git, repo as init_repo};

    #[test]
    fn a_remote_s_url_from_the_config_file() {
        let config = "[core]\n\tbare = false\n[remote \"upstream\"]\n\turl = git@github.com:other/x.git\n\
                      [remote \"origin\"]\n\turl = git@github.com:acme/widgets.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n";
        assert_eq!(
            remote_url(config, "origin").as_deref(),
            Some("git@github.com:acme/widgets.git")
        );
        assert_eq!(remote_url("[remote \"origin\"]\n", "origin"), None);
    }

    #[test]
    fn a_fork_s_upstream_is_read_beside_its_origin() {
        let config = |upstream: Option<&str>| {
            let mut text = "[core]\n\tbare = false\n[remote \"origin\"]\n\turl = git@github.com:me/widgets.git\n\
                            \tfetch = +refs/heads/*:refs/remotes/origin/*\n"
                .to_string();
            if let Some(url) = upstream {
                text.push_str(&format!(
                    "[remote \"upstream\"]\n\turl = {url}\n\tfetch = +refs/heads/*:refs/remotes/upstream/*\n"
                ));
            }
            text
        };
        let upstream = |upstream: Option<&str>| {
            let remotes = remotes_in(&config(upstream));
            assert_eq!(
                remotes.origin.as_ref().map(|r| r.to_string()).as_deref(),
                Some("me/widgets")
            );
            remotes.upstream.map(|r| r.to_string())
        };
        // Present, as https or ssh writes it.
        assert_eq!(
            upstream(Some("https://github.com/acme/widgets.git")).as_deref(),
            Some("acme/widgets")
        );
        assert_eq!(
            upstream(Some("git@github.com:acme/widgets.git")).as_deref(),
            Some("acme/widgets")
        );
        assert_eq!(
            upstream(Some("ssh://git@github.com/acme/widgets")).as_deref(),
            Some("acme/widgets")
        );
        // Absent, elsewhere than GitHub, or origin itself however written.
        assert_eq!(upstream(None), None);
        assert_eq!(upstream(Some("git@gitlab.com:acme/widgets.git")), None);
        assert_eq!(upstream(Some("https://github.com/me/widgets")), None);
        assert_eq!(upstream(Some("git@github.com:Me/Widgets.git")), None);
        // Another remote is no upstream, and an upstream with no origin on
        // GitHub to be a fork of is none either.
        let other = "[remote \"origin\"]\n\turl = git@github.com:me/widgets.git\n\
                     [remote \"fork\"]\n\turl = git@github.com:acme/widgets.git\n";
        assert_eq!(remotes_in(other).upstream, None);
        let lone = "[remote \"upstream\"]\n\turl = git@github.com:acme/widgets.git\n";
        assert_eq!(remotes_in(lone), Remotes::default());
        let elsewhere = "[remote \"origin\"]\n\turl = git@gitlab.com:me/widgets.git\n\
                         [remote \"upstream\"]\n\turl = git@github.com:acme/widgets.git\n";
        assert_eq!(remotes_in(elsewhere), Remotes::default());
    }

    #[test]
    fn a_fork_s_clone_reads_both_from_its_own_config() {
        let scratch = Scratch::new("upstream");
        let dir = scratch.0.join("widgets");
        std::fs::create_dir_all(&dir).unwrap();
        if !init_repo(&dir) {
            return;
        }
        assert!(run_git(
            &dir,
            &["remote", "add", "origin", "git@github.com:me/widgets.git"]
        ));
        assert_eq!(remotes_of(&dir).upstream, None);
        assert!(run_git(
            &dir,
            &[
                "remote",
                "add",
                "upstream",
                "https://github.com/acme/widgets.git"
            ]
        ));
        let remotes = remotes_of(&dir);
        let all: Vec<String> = remotes.all().map(|r| r.to_string()).collect();
        assert_eq!(all, ["me/widgets", "acme/widgets"]);
        assert_eq!(
            origin_of(&dir).map(|r| r.to_string()).as_deref(),
            Some("me/widgets")
        );
        assert_eq!(remotes_of(&scratch.0.join("nowhere")), Remotes::default());
    }

    #[test]
    fn a_repository_is_found_from_its_origin_in_every_form_git_takes() {
        // lib/sidebar's "a repository is found from its origin in every
        // form git takes, SSH over port 443 included".
        let remotes = [
            ("https://github.com/acme/widgets.git", Some("acme/widgets")),
            ("https://github.com/acme/widgets", Some("acme/widgets")),
            ("git@github.com:acme/widgets.git", Some("acme/widgets")),
            (
                "ssh://git@github.com/acme/widgets.git",
                Some("acme/widgets"),
            ),
            (
                "ssh://git@ssh.github.com:443/acme/widgets.git",
                Some("acme/widgets"),
            ),
            ("git@github.com:1234/widgets.git", Some("1234/widgets")),
            ("git@gitlab.com:acme/widgets.git", None),
            ("https://gist.github.com/acme/0123abcd.git", None),
        ];
        let scratch = Scratch::new("origin-forms");
        for (i, (url, want)) in remotes.iter().enumerate() {
            let dir = scratch.0.join(format!("r{i}"));
            std::fs::create_dir_all(&dir).unwrap();
            if !init_repo(&dir) {
                return;
            }
            assert!(run_git(&dir, &["remote", "add", "origin", url]));
            assert_eq!(
                origin_of(&dir).map(|r| r.to_string()).as_deref(),
                *want,
                "{url}"
            );
        }
    }

    #[test]
    fn a_tab_in_a_removed_worktree_takes_its_branch_s_pr_unless_its_name_is_about_another_ticket() {
        let repo = Repo {
            owner: "acme".into(),
            name: "gone".into(),
        };
        let gone = Path::new("/nowhere/gone-repo/.claude/worktrees/abc-5-old-work");
        let mut remotes = |_: &Path| Remotes {
            origin: Some(repo.clone()),
            upstream: None,
        };
        let want = vec![PrKey::Branch {
            repo: repo.clone(),
            branch: "abc-5-old-work".into(),
            from: None,
        }];
        assert_eq!(
            keys_of("ABC-5: Old work", Some(gone), None, None, &mut remotes),
            want,
            "its own ticket"
        );
        assert_eq!(
            keys_of("a plain name", Some(gone), None, None, &mut remotes),
            want,
            "no ticket named"
        );
        assert_eq!(
            keys_of(
                "Shaping ABC-9: New work",
                Some(gone),
                None,
                None,
                &mut remotes
            ),
            Vec::new(),
            "shaping another ticket now"
        );
        // A tab with a PR is asked about by its number.
        let keys: Vec<String> = keys_of(
            "x",
            Some(Path::new("/w")),
            Some(12),
            Some(&repo),
            &mut remotes,
        )
        .iter()
        .map(|k| k.to_string())
        .collect();
        assert_eq!(keys, ["acme/gone#12"]);
        // A worktree that is still there is no gone one.
        assert_eq!(gone_worktree(Path::new("/")), None);
    }

    #[test]
    fn a_fork_s_branch_is_asked_of_origin_then_of_upstream_from_the_fork() {
        let remotes = Remotes {
            origin: github::parse_remote("git@github.com:me/widgets.git"),
            upstream: github::parse_remote("https://github.com/acme/widgets.git"),
        };
        let keys: Vec<String> = branch_keys(&remotes, "feature")
            .iter()
            .map(|k| k.to_string())
            .collect();
        assert_eq!(keys, ["me/widgets@feature", "acme/widgets@me:feature"]);
        assert!(branch_keys(&remotes, "feature").iter().all(PrKey::safe));
        assert_eq!(branch_keys(&Remotes::default(), "feature"), Vec::new());
        // A gone worktree in a fork's clone is asked about both ways.
        let gone = Path::new("/nowhere/widgets/.claude/worktrees/feature");
        let keys = keys_of("x", Some(gone), None, None, &mut |_| remotes.clone());
        assert_eq!(keys, branch_keys(&remotes, "feature"));
    }

    #[test]
    fn only_real_names_are_keys() {
        for key in [
            "acme/widgets#7",
            "acme/widgets@feat-x",
            "acme/widgets@me:feat/x",
        ] {
            assert_eq!(
                PrKey::parse(key).map(|k| k.to_string()).as_deref(),
                Some(key)
            );
        }
        for bad in [
            "acme/widgets#1) { x } #",
            "a\"b/c#2",
            "acme/widgets@x\" }",
            "acme#3",
        ] {
            assert_eq!(PrKey::parse(bad), None, "{bad}");
        }
        let unsafe_branch = PrKey::Branch {
            repo: Repo {
                owner: "acme".into(),
                name: "w".into(),
            },
            branch: "x\" } y".into(),
            from: None,
        };
        assert!(!unsafe_branch.safe());
        let unsafe_fork = PrKey::Branch {
            repo: Repo {
                owner: "acme".into(),
                name: "w".into(),
            },
            branch: "x".into(),
            from: Some("me\" } y".into()),
        };
        assert!(!unsafe_fork.safe());
    }
}
