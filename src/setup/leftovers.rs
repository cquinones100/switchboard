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
//!   init zsh)"` and the rest, with the comments it wrote above them: each
//!   is taken out, since every tab, menu and skill that runs a worktree
//!   function defines it itself; the same in `~/.bashrc`.
//! - A `switchboard` shell function or alias that starts tmux: the tmux
//!   version's `tmux_server` setting ran it on a server of its own, opened
//!   with `tmux -L switchboard new-session -A -s main`, which people wrapped
//!   in a function or alias of that name. Left in, it runs in the binary's
//!   place in every new shell, so `switchboard` still starts tmux. It is
//!   taken out, with the comment right above it when that names Switchboard
//!   and tmux.
//!
//! A startup file is written through a link to it (a dotfiles repository's),
//! and a copy of it as it was is left beside it.
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

/// A startup file's text without the tmux version's worktree lines, or
/// `None` when it has none. A line is one when it evals a worktree script's
/// `init` by the script's name or a path to it, as setup.sh wrote them;
/// nothing needs them now, since every tab, menu and skill that runs `wt`,
/// `wtn` and the rest defines the function itself, so they go whether or
/// not the script still runs. The comment lines right above one go too when
/// they name the function it defined (`wt`, `wtpr`, `wtr`, `wtb`, `wtn`),
/// as setup.sh's words said what each was for. A line running the binary's
/// own `init` (`switchboard worktree-from-note init zsh`) is yours, added to
/// type the functions, and left alone.
pub fn rc_cleared(text: &str) -> Option<String> {
    let line = Regex::new(&format!(
        r#"^\s*eval\s+"\$\((?:[^"\s()]*/)?(?:{})\s+init\s+(?:zsh|bash)\s*\)"\s*(?:#.*)?$"#,
        WORKTREE_SCRIPTS.join("|")
    ))
    .expect("a valid pattern");
    let names = Regex::new(r"`(?:wt|wtpr|wtr|wtb|wtn)`").expect("a valid pattern");
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut keep = vec![true; lines.len()];
    for (i, l) in lines.iter().enumerate() {
        if !line.is_match(l.trim_end_matches('\n')) {
            continue;
        }
        keep[i] = false;
        // The comment block right above, when it names a function.
        let mut top = i;
        while top > 0 && lines[top - 1].trim_start().starts_with('#') {
            top -= 1;
        }
        if top < i && lines[top..i].iter().any(|c| names.is_match(c)) {
            for k in keep.iter_mut().take(i).skip(top) {
                *k = false;
            }
        }
    }
    if keep.iter().all(|k| *k) {
        return None;
    }
    let kept: Vec<&str> = lines
        .iter()
        .zip(&keep)
        .filter(|(_, k)| **k)
        .map(|(l, _)| *l)
        .collect();
    // No run of blank lines left longer than the file had.
    let mut out = String::new();
    let mut blanks = 0;
    for l in kept {
        if l.trim().is_empty() {
            blanks += 1;
            if blanks > 1 {
                continue;
            }
        } else {
            blanks = 0;
        }
        out.push_str(l);
    }
    Some(out)
}

/// A startup file's text without a `switchboard` function or alias that
/// starts tmux, or `None` when it has none. Only a definition on one line
/// whose body runs tmux goes, as the tmux version's command was wrapped; one
/// of your own that runs anything else is left alone. A comment line right
/// above it goes too when it names Switchboard and tmux, as it would say
/// what the definition was for.
pub fn rc_unshadowed(text: &str) -> Option<String> {
    let definition = Regex::new(
        r#"^\s*(?:(?:function\s+switchboard(?:\s*\(\))?|switchboard\s*\(\))\s*\{.*\btmux\b.*\}|alias\s+switchboard=(?:'[^']*\btmux\b[^']*'|"[^"]*\btmux\b[^"]*"))\s*;?\s*(?:#.*)?$"#,
    )
    .expect("a valid pattern");
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut keep = vec![true; lines.len()];
    for (i, l) in lines.iter().enumerate() {
        if !definition.is_match(l.trim_end_matches('\n')) {
            continue;
        }
        keep[i] = false;
        if let Some(above) = i.checked_sub(1).map(|j| lines[j].trim().to_lowercase())
            && above.starts_with('#')
            && above.contains("switchboard")
            && above.contains("tmux")
        {
            keep[i - 1] = false;
        }
    }
    if keep.iter().all(|k| *k) {
        return None;
    }
    Some(
        lines
            .iter()
            .zip(&keep)
            .filter(|(_, k)| **k)
            .map(|(l, _)| *l)
            .collect(),
    )
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
        let mut fixed = text;
        let mut done = Vec::new();
        if let Some(t) = rc_cleared(&fixed) {
            fixed = t;
            done.push("take out the tmux version's worktree lines");
        }
        if let Some(t) = rc_unshadowed(&fixed) {
            fixed = t;
            done.push("take out the switchboard function or alias that starts tmux");
        }
        if done.is_empty() {
            continue;
        }
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
        r.change(format!("{} in {}", done.join(", and "), home_tilde(&rc)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tmux_versions_worktree_lines_are_taken_out() {
        let text = "export X=1\n\
                    \n\
                    # Defines `wt`: creates a worktree from a Linear ticket, opens claude\n\
                    # in it. Linear key: ~/.config/worktree-from-ticket/config.json\n\
                    eval \"$(worktree-from-ticket init zsh)\"\n\
                    \n\
                    # my own note\n\
                    eval \"$(~/.local/bin/worktree-from-pr init bash)\" # mine\n\
                    eval \"$(switchboard worktree-resume init zsh)\"\n\
                    \n\
                    export Y=2\n";
        assert_eq!(
            rc_cleared(text).unwrap(),
            "export X=1\n\
             \n\
             # my own note\n\
             eval \"$(switchboard worktree-resume init zsh)\"\n\
             \n\
             export Y=2\n"
        );
        assert_eq!(
            rc_cleared("eval \"$(switchboard worktree-from-note init zsh)\"\n"),
            None
        );
        assert_eq!(rc_cleared("# nothing here\n"), None);
    }

    #[test]
    fn a_switchboard_function_or_alias_that_starts_tmux_is_taken_out() {
        let text = "export X=1\n\
                    # Switchboard runs on a tmux server of its own: `switchboard` opens it.\n\
                    switchboard() { command tmux -L switchboard new-session -A -s main \"$@\"; }\n\
                    # A plain `tmux` starts the default server.\n\
                    alias tmux='tmux -f ~/.tmux.conf'\n";
        assert_eq!(
            rc_unshadowed(text).unwrap(),
            "export X=1\n\
             # A plain `tmux` starts the default server.\n\
             alias tmux='tmux -f ~/.tmux.conf'\n"
        );
        // The other ways to write one, a comment of your own above kept.
        for line in [
            "function switchboard { tmux -L switchboard attach; }",
            "function switchboard() { tmux new -A -s main }",
            "alias switchboard='tmux -L switchboard new-session -A -s main'",
            "  alias switchboard=\"tmux -L switchboard attach\" # mine",
        ] {
            assert_eq!(
                rc_unshadowed(&format!("# my shortcuts\n{line}\nexport Y=2")).as_deref(),
                Some("# my shortcuts\nexport Y=2"),
                "{line}"
            );
        }
        // One of your own that doesn't start tmux, and the binary's lines,
        // are left alone.
        for kept in [
            "switchboard() { command switchboard -s work \"$@\"; }",
            "alias switchboard='switchboard -s main'",
            "alias sb='tmux -L switchboard attach'",
            "eval \"$(switchboard worktree-from-ticket init zsh)\"",
        ] {
            assert_eq!(rc_unshadowed(&format!("{kept}\n")), None, "{kept}");
        }
    }
}
