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
    Number { repo: Repo, number: u64 },
    Branch { repo: Repo, branch: String },
}

impl PrKey {
    pub fn repo(&self) -> &Repo {
        match self {
            PrKey::Number { repo, .. } | PrKey::Branch { repo, .. } => repo,
        }
    }

    /// prs.js's key, `owner/name#123` or `owner/name@branch`, read back,
    /// only when every part is something GitHub could name, since the parts
    /// go into a query's text.
    pub fn parse(key: &str) -> Option<PrKey> {
        static KEY: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"^([\w.-]+)/([\w.-]+)(?:#(\d+)|@([\w./-]+))$").unwrap());
        let at = KEY.captures(key)?;
        let repo = Repo {
            owner: at[1].to_string(),
            name: at[2].to_string(),
        };
        match (at.get(3), at.get(4)) {
            (Some(n), _) => Some(PrKey::Number {
                repo,
                number: n.as_str().parse().ok()?,
            }),
            (_, Some(b)) => Some(PrKey::Branch {
                repo,
                branch: b.as_str().to_string(),
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
                PrKey::Branch { branch, .. } => BRANCH.is_match(branch),
            }
    }
}

impl fmt::Display for PrKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PrKey::Number { repo, number } => write!(f, "{repo}#{number}"),
            PrKey::Branch { repo, branch } => write!(f, "{repo}@{branch}"),
        }
    }
}

/// The GitHub repository `dir` belongs to, from its `origin` remote, or
/// `None` outside a repository or for an origin elsewhere.
pub fn origin_of(dir: &Path) -> Option<Repo> {
    let place = git::locate(dir)?;
    let config = fs::read_to_string(place.common_dir.join("config")).ok()?;
    github::parse_remote(&remote_url(&config, "origin")?)
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

/// What a tab's PR is asked for by: its PR in the repository it is in
/// (`repo`: where the tab names' lookup found it, else the tab's origin),
/// else a gone worktree's branch, unless its name is about another ticket.
/// `origin` reads a directory's repository (the server's cache of
/// `origin_of`).
pub fn key_of(
    name: &str,
    path: Option<&Path>,
    pr: Option<u64>,
    repo: Option<&Repo>,
    origin: &mut dyn FnMut(&Path) -> Option<Repo>,
) -> Option<PrKey> {
    if let (Some(repo), Some(number)) = (repo, pr) {
        return Some(PrKey::Number {
            repo: repo.clone(),
            number,
        });
    }
    let (root, branch) = gone_worktree(path?)?;
    if elsewhere(name, &branch) {
        return None;
    }
    Some(PrKey::Branch {
        repo: origin(&root)?,
        branch,
    })
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
        let mut origin = |_: &Path| Some(repo.clone());
        let want = Some(PrKey::Branch {
            repo: repo.clone(),
            branch: "abc-5-old-work".into(),
        });
        assert_eq!(
            key_of("ABC-5: Old work", Some(gone), None, None, &mut origin),
            want,
            "its own ticket"
        );
        assert_eq!(
            key_of("a plain name", Some(gone), None, None, &mut origin),
            want,
            "no ticket named"
        );
        assert_eq!(
            key_of(
                "Shaping ABC-9: New work",
                Some(gone),
                None,
                None,
                &mut origin
            ),
            None,
            "shaping another ticket now"
        );
        // A tab with a PR is asked about by its number.
        assert_eq!(
            key_of(
                "x",
                Some(Path::new("/w")),
                Some(12),
                Some(&repo),
                &mut origin
            )
            .map(|k| k.to_string())
            .as_deref(),
            Some("acme/gone#12")
        );
        // A worktree that is still there is no gone one.
        assert_eq!(gone_worktree(Path::new("/")), None);
    }

    #[test]
    fn only_real_names_are_keys() {
        assert_eq!(
            PrKey::parse("acme/widgets#7").map(|k| k.to_string()),
            Some("acme/widgets#7".into())
        );
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
        };
        assert!(!unsafe_branch.safe());
    }
}
