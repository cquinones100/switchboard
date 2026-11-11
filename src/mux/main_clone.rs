//! The main clone of a checkout, read from git's own files, for build.rs
//! to record as `SWITCHBOARD_MAIN_CLONE` (it includes this file by its
//! path, so it uses nothing but std) and M-p's restart to build
//! (src/mux/latest.rs). A worktree is removed once its work lands, so the
//! checkout a build ran in may be gone by the next restart; the main clone
//! stays, and once the worktree's work has landed it is what counts.
//!
//! No git is run: a worktree's `.git` is a file, `gitdir: <main>/.git/
//! worktrees/<name>`, whose `commondir` leads to `<main>/.git`; the main
//! clone's own is a directory.

use std::path::{Path, PathBuf};

/// The main worktree of the clone `checkout` belongs to (`checkout` itself
/// when it is the main one), or `None` when `checkout` isn't in git or its
/// clone is bare.
pub fn of(checkout: &Path) -> Option<PathBuf> {
    let dot_git = checkout.join(".git");
    if dot_git.is_dir() {
        return Some(checkout.to_path_buf());
    }
    let text = std::fs::read_to_string(&dot_git).ok()?;
    let gitdir = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
    let gitdir = checkout.join(gitdir);
    let common = std::fs::read_to_string(gitdir.join("commondir")).ok()?;
    let common = gitdir.join(common.trim()).canonicalize().ok()?;
    if common.file_name()? != ".git" {
        return None;
    }
    common.parent().map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-C"])
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn a_worktree_names_its_main_clone_and_the_main_clone_itself() {
        let dir = std::env::temp_dir().join(format!("sb-main-clone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let main = dir.join("main");
        std::fs::create_dir_all(&main).unwrap();
        git(&main, &["init", "-q", "-b", "main"]);
        git(&main, &["commit", "-q", "--allow-empty", "-m", "one"]);
        let wt = main.join(".claude/worktrees/item");
        git(
            &main,
            &["worktree", "add", "-q", "-b", "item", wt.to_str().unwrap()],
        );
        let main = main.canonicalize().unwrap();
        assert_eq!(of(&main), Some(main.clone()));
        assert_eq!(of(&wt), Some(main.clone()));
        assert_eq!(of(&dir), None, "not in git");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
