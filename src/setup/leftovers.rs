//! The `leftovers` part: what the tmux version's setup.sh put in your home
//! that leads nowhere once its clone is pulled past the rewrite, which
//! removed the files it linked to.
//!
//! - Links leading nowhere into a clone's `bin/`, `tmux/` or `claude/`:
//!   setup.sh's `part_bin` linked each script into `~/.local/bin`, its
//!   `tmux` part each script into `~/.config/tmux`, and its `claude` part
//!   `~/.claude/commands`, `~/.claude/hooks/*`, the statusline and the OTel
//!   headers script. Each is removed; a link that still leads somewhere, or
//!   into anything but a Switchboard clone, is yours and left alone. (The
//!   skills part replaces the old links in `~/.claude/skills` itself.)
//! - The lines setup.sh added to `~/.zshrc`, `eval "$(worktree-from-ticket
//!   init zsh)"` and the rest, which fail on every new shell once the script
//!   is gone: each is turned into the same line run through the binary
//!   (`switchboard worktree-from-ticket init zsh`), so the functions it
//!   defined are still there; the same in `~/.bashrc`. The file is written
//!   through a link to it (a dotfiles repository's), and a copy of it as it
//!   was is left beside it.
//!
//! As every part, a second run finds nothing to do.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use regex::Regex;

use super::{Report, home_tilde, is_link, stamp};
use crate::{paths, util};

/// The worktree commands setup.sh's `~/.zshrc` lines ran by their scripts'
/// names.
const WORKTREE_SCRIPTS: &[&str] = &[
    "worktree-from-ticket",
    "worktree-from-pr",
    "worktree-resume",
    "worktree-from-branch",
    "worktree-from-note",
];

/// The folders setup.sh linked files into, each looked through (not below).
fn link_dirs() -> Vec<PathBuf> {
    vec![
        paths::home_join(".local/bin"),
        paths::home_join(".config/tmux"),
        paths::claude_dir().join("hooks"),
    ]
}

/// The single links setup.sh made in `~/.claude`.
fn link_files() -> Vec<PathBuf> {
    let claude = paths::claude_dir();
    vec![
        claude.join("commands"),
        claude.join("statusline-command.sh"),
        claude.join("otel-headers.sh"),
        claude.join("hooks"),
    ]
}

/// The shell startup files setup.sh (or someone following its words) wrote
/// the worktree functions into.
fn rc_files() -> Vec<PathBuf> {
    vec![paths::home_join(".zshrc"), paths::home_join(".bashrc")]
}

/// Whether `link` is a link leading nowhere into a Switchboard clone's
/// `bin/`, `tmux/` or `claude/`: the folder above that is a clone (the tmux
/// version's, or the Rust version's, which has no such files), or is gone
/// and was named as a clone is.
pub fn dead_clone_link(link: &Path) -> bool {
    if !is_link(link) || link.exists() {
        return false;
    }
    let Ok(target) = fs::read_link(link) else {
        return false;
    };
    // A relative link leads from the folder it is in.
    let target = match link.parent() {
        Some(dir) => dir.join(&target),
        None => target,
    };
    let parts: Vec<_> = target.components().collect();
    (1..parts.len()).any(|i| {
        let name = parts[i].as_os_str();
        if name != "bin" && name != "tmux" && name != "claude" {
            return false;
        }
        let clone: PathBuf = parts[..i].iter().collect();
        is_clone(&clone)
    })
}

/// A Switchboard clone: the tmux version's (setup.sh and tmux.conf), the
/// Rust version's (Cargo.toml and the skills it compiles in), or one gone
/// whose folder had a clone's name.
fn is_clone(dir: &Path) -> bool {
    if dir.exists() {
        return (dir.join("setup.sh").is_file() && dir.join("tmux/tmux.conf").is_file())
            || (dir.join("Cargo.toml").is_file() && dir.join("assets/skills").is_dir());
    }
    dir.file_name().is_some_and(|n| {
        let n = n.to_string_lossy();
        n == "switchboard" || n == "editor-configs"
    })
}

/// Every link of setup.sh's that leads nowhere now.
pub fn dead_links() -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = link_files()
        .into_iter()
        .filter(|p| dead_clone_link(p))
        .collect();
    for dir in link_dirs() {
        // A folder that is itself a link is setup.sh's single link, above.
        if is_link(&dir) {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut here: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| dead_clone_link(p))
            .collect();
        here.sort();
        found.extend(here);
    }
    found.dedup();
    found
}

/// A startup file's text with each leftover line run through the binary,
/// or `None` when it has none. A line is a leftover when it evals a worktree
/// script's `init` by a name or path that no longer runs anything.
pub fn rc_fixed(text: &str) -> Option<String> {
    let line = Regex::new(&format!(
        r#"^(\s*eval\s+"\$\()((?:[^"\s()]*/)?({}))(\s+init\s+(?:zsh|bash)\s*\)".*)$"#,
        WORKTREE_SCRIPTS.join("|")
    ))
    .expect("a valid pattern");
    let mut changed = false;
    let fixed: Vec<String> = text
        .split_inclusive('\n')
        .map(|l| {
            let (body, end) = match l.strip_suffix('\n') {
                Some(body) => (body, "\n"),
                None => (l, ""),
            };
            let Some(c) = line.captures(body) else {
                return l.to_string();
            };
            let named = &c[2];
            let runs = if named.contains('/') {
                let expanded = match named.strip_prefix("~/") {
                    Some(rest) => paths::home_join(rest),
                    None => match named.strip_prefix("$HOME/") {
                        Some(rest) => paths::home_join(rest),
                        None => PathBuf::from(named),
                    },
                };
                util::which(&expanded.to_string_lossy()).is_some()
            } else {
                util::which(named).is_some()
            };
            if runs {
                return l.to_string();
            }
            changed = true;
            format!("{}switchboard {}{}{end}", &c[1], &c[3], &c[4])
        })
        .collect();
    changed.then(|| fixed.concat())
}

/// The part.
pub fn run(r: &mut Report) -> Result<()> {
    for link in dead_links() {
        if !r.check {
            fs::remove_file(&link)
                .with_context(|| format!("can't remove {}", home_tilde(&link)))?;
        }
        r.change(format!(
            "remove {}, which led to a file the tmux version had",
            home_tilde(&link)
        ));
    }
    for rc in rc_files() {
        let Some(text) = util::read_text(&rc) else {
            continue;
        };
        let Some(fixed) = rc_fixed(&text) else {
            continue;
        };
        if !r.check {
            // Through a link to the file, so a dotfiles repository's copy
            // is what changes, and the link stays a link.
            let real = fs::canonicalize(&rc).unwrap_or_else(|_| rc.clone());
            let name = real
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let copy = real.with_file_name(format!("{name}.bak.{}", stamp()));
            fs::copy(&real, &copy).with_context(|| format!("can't copy {}", home_tilde(&real)))?;
            let mode = fs::metadata(&real).map(|m| m.permissions());
            util::write_atomic(&real, &fixed)?;
            if let Ok(mode) = mode {
                let _ = fs::set_permissions(&real, mode);
            }
        }
        r.change(format!(
            "run the worktree functions' init through switchboard in {}",
            home_tilde(&rc)
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_whose_script_is_gone_runs_through_the_binary() {
        let text = "export X=1\n\
                    eval \"$(worktree-from-ticket init zsh)\"\n\
                    eval \"$(~/.local/bin/worktree-from-pr init bash)\" # mine\n\
                    eval \"$(switchboard worktree-resume init zsh)\"\n";
        assert_eq!(
            rc_fixed(text).unwrap(),
            "export X=1\n\
             eval \"$(switchboard worktree-from-ticket init zsh)\"\n\
             eval \"$(switchboard worktree-from-pr init bash)\" # mine\n\
             eval \"$(switchboard worktree-resume init zsh)\"\n"
        );
        assert_eq!(rc_fixed("eval \"$(switchboard wt init zsh)\"\n"), None);
        assert_eq!(rc_fixed("# nothing here\n"), None);
    }
}
