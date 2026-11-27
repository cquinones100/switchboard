//! Whether a directory is Switchboard's own clone, the one place that
//! decides it, as `src/trust.rs` alone decides whose code a directory holds.
//! Switchboard is developed with itself, and what serves only that work
//! (leak-check before something reaches the public repository, the
//! settings and the restart for a build made from its source) is kept to
//! its clone, so a user working on their own repositories never meets it.
//!
//! A directory is Switchboard's clone when it is the top of a checkout (the
//! main clone or any of its worktrees) holding Switchboard's sources, a
//! `Cargo.toml` naming the `switchboard` package with `assets/skills` and
//! `src/mux` beside it, and its `origin` is a GitHub repository named
//! `switchboard` (any owner, so a fork counts; or `editor-configs`, its old
//! name, in any case), read from git's own config file with no git run,
//! since the running build's answer is asked on the drawing path (the
//! palette, Settings).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::util;

/// Whether `dir` is in Switchboard's clone or one of its worktrees, as the
/// module's comment says: the checkout's top is the nearest folder holding
/// `.git`, so a folder inside it counts too.
pub fn is_switchboard_clone(dir: &Path) -> bool {
    let Some(top) = dir.ancestors().find(|d| d.join(".git").exists()) else {
        return false;
    };
    has_sources(top) && origin_url(top).is_some_and(|url| names_switchboard(&url))
}

/// `is_switchboard_clone`, answered once a process for each directory, for
/// what asks on the drawing path (autopilot's board), where no file is read
/// again: a checkout's being Switchboard's clone doesn't change while it runs.
pub fn is_switchboard_clone_remembered(dir: &Path) -> bool {
    static SEEN: std::sync::Mutex<Vec<(PathBuf, bool)>> = std::sync::Mutex::new(Vec::new());
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, is)) = seen.iter().find(|(d, _)| d == dir) {
        return *is;
    }
    let is = is_switchboard_clone(dir);
    seen.push((dir.to_path_buf(), is));
    is
}

/// Whether the running build was made from Switchboard's clone: the
/// checkout M-p's restart would build (`mux::latest::checkout`, none for a
/// release build), decided once a process, since it stays the same while
/// the process runs.
pub fn build_is_from_clone() -> bool {
    static FROM_CLONE: OnceLock<bool> = OnceLock::new();
    *FROM_CLONE.get_or_init(|| {
        crate::mux::latest::checkout().is_some_and(|c| is_switchboard_clone(&c.dir))
    })
}

/// Switchboard's sources: the package's manifest and two of its folders.
fn has_sources(dir: &Path) -> bool {
    let named = util::read_text(dir.join("Cargo.toml")).is_some_and(|toml| {
        toml.lines()
            .any(|l| l.replace(' ', "") == "name=\"switchboard\"")
    });
    named && dir.join("assets/skills").is_dir() && dir.join("src/mux").is_dir()
}

/// The config file git reads for the checkout at `dir`: a main clone's
/// `.git/config`, or, for a worktree (whose `.git` is a file naming its
/// git directory), the config of the clone it belongs to.
fn config_of(dir: &Path) -> Option<PathBuf> {
    let dot_git = dir.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git.join("config"));
    }
    let text = util::read_text(&dot_git)?;
    let gitdir = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
    let gitdir = dir.join(gitdir);
    let common = match util::read_text(gitdir.join("commondir")) {
        Some(common) => gitdir.join(common.trim()),
        None => gitdir,
    };
    Some(common.join("config"))
}

/// The `url` of `[remote "origin"]` in the checkout's git config.
fn origin_url(dir: &Path) -> Option<String> {
    let config = util::read_text(config_of(dir)?)?;
    let mut in_origin = false;
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_origin = line.replace(' ', "") == "[remote\"origin\"]";
            continue;
        }
        if in_origin
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == "url"
        {
            return Some(value.trim().trim_matches('"').to_string());
        }
    }
    None
}

/// Whether a remote's URL is a GitHub repository named `switchboard`, or
/// `editor-configs`, its name until October 2026, which a clone made before
/// the rename still reads and GitHub still answers:
/// `git@github.com:<owner>/switchboard.git`, `https://github.com/<owner>/switchboard`,
/// `ssh://git@github.com/<owner>/switchboard.git`, with or without `.git`,
/// in any case, as GitHub takes them.
fn names_switchboard(url: &str) -> bool {
    let url = url.trim().trim_end_matches('/').to_ascii_lowercase();
    let url = url.strip_suffix(".git").unwrap_or(&url);
    let path = if let Some(rest) = url.strip_prefix("git@github.com:") {
        rest
    } else {
        let Some((_, rest)) = url.split_once("://") else {
            return false;
        };
        let rest = rest.rsplit_once('@').map_or(rest, |(_, host)| host);
        let Some(rest) = rest.strip_prefix("github.com/") else {
            return false;
        };
        rest
    };
    matches!(
        path.split('/').collect::<Vec<_>>().as_slice(),
        [owner, "switchboard" | "editor-configs"] if !owner.is_empty()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sb-dev-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn clone_at(dir: &Path, origin: &str) {
        std::fs::create_dir_all(dir.join("assets/skills")).unwrap();
        std::fs::create_dir_all(dir.join("src/mux")).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"switchboard\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(
            dir.join(".git/config"),
            format!(
                "[core]\n\tbare = false\n[remote \"upstream\"]\n\turl = https://github.com/acme/other.git\n[remote \"origin\"]\n\turl = {origin}\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n"
            ),
        )
        .unwrap();
    }

    #[test]
    fn a_clone_and_its_worktree_are_switchboards() {
        let main = scratch("main").join("main");
        clone_at(&main, "git@github.com:acme/switchboard.git");
        assert!(is_switchboard_clone(&main));
        assert!(
            is_switchboard_clone(&main.join("src/mux")),
            "a folder in it"
        );

        let wt = scratch("wt").join("wt");
        std::fs::create_dir_all(wt.join("assets/skills")).unwrap();
        std::fs::create_dir_all(wt.join("src/mux")).unwrap();
        std::fs::copy(main.join("Cargo.toml"), wt.join("Cargo.toml")).unwrap();
        let gitdir = main.join(".git/worktrees/wt");
        std::fs::create_dir_all(&gitdir).unwrap();
        std::fs::write(gitdir.join("commondir"), "../..\n").unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
        assert!(is_switchboard_clone(&wt));
    }

    #[test]
    fn a_look_alike_with_another_origin_or_missing_sources_is_not() {
        let other = scratch("other").join("other");
        clone_at(&other, "https://github.com/acme/widgets.git");
        assert!(!is_switchboard_clone(&other), "another repository");

        let local = scratch("local").join("local");
        clone_at(&local, "/tmp/origin.git");
        assert!(!is_switchboard_clone(&local), "an origin that isn't GitHub");

        let partial = scratch("partial").join("partial");
        clone_at(&partial, "https://github.com/acme/switchboard");
        std::fs::remove_dir_all(partial.join("src/mux")).unwrap();
        assert!(!is_switchboard_clone(&partial), "missing its sources");

        let none = scratch("none").join("none");
        std::fs::create_dir_all(&none).unwrap();
        assert!(!is_switchboard_clone(&none), "not a checkout");
    }

    #[test]
    fn github_urls_naming_switchboard() {
        for url in [
            "git@github.com:acme/switchboard.git",
            "git@github.com:acme/switchboard",
            "https://github.com/acme/switchboard.git",
            "https://github.com/acme/switchboard/",
            "ssh://git@github.com/acme/switchboard.git",
            "https://github.com/acme/editor-configs.git",
            "git@GitHub.com:Acme/Switchboard.git",
        ] {
            assert!(names_switchboard(url), "{url}");
        }
        for url in [
            "https://github.com/acme/switchboard-notes.git",
            "https://gitlab.com/acme/switchboard.git",
            "https://github.com/switchboard",
            "https://github.com/acme/switchboard/extra",
            "/tmp/switchboard.git",
        ] {
            assert!(!names_switchboard(url), "{url}");
        }
    }
}
