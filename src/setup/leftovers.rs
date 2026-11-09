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
//!   One Claude Code still uses is put back as a file instead: a link that
//!   `~/.claude/settings.json` names (a hook's command, `otelHeadersHelper`;
//!   not the statusline, which the `claude` part moves onto `switchboard
//!   statusline`), and `~/.claude/commands`, which Claude reads by
//!   itself, are each replaced with a copy of what they led to, taken from
//!   the clone's history (the commit before the one that removed it). Removed
//!   instead, the guard hooks, the OTel headers and the commands would stop
//!   without a word. One that can't be found there is left alone, and said.
//! - Links leading nowhere into any other path in a clone, where setup.sh
//!   linked the configs the clone kept until the rewrite removed them
//!   (`~/.config/ghostty/config`, `~/.config/karabiner/karabiner.json`,
//!   `~/.pgclirc`, lazygit's `config.yml`, the `~/.config/nvim` folder) and
//!   anywhere else one might have been: directly in `~`, `~/.config` and the
//!   folders in it, `~/Library/Application Support` and the folders in it,
//!   and directly in `~/.claude`. Each is replaced with what it last held in
//!   the clone's history, a folder as it was in the commit before the last
//!   one that removed anything in it, and the part names it. A Ghostty
//!   config's `command` that started tmux starts the binary instead. A link into
//!   the clone's older name, `editor-configs`, gone since the rename, is
//!   looked up in the `switchboard` clone beside it, and removed when that
//!   history doesn't have it either.
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

use serde_json::Value;

use super::{Report, claude_settings, home_tilde, is_link, stamp};
use crate::{paths, util, worktree};

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
    clone_target(link).is_some()
}

/// The clone a link leading nowhere into its `bin/`, `tmux/` or `claude/`
/// led into, and the path it led to inside it (`claude/hooks/enforce.sh`).
fn clone_target(link: &Path) -> Option<(PathBuf, PathBuf)> {
    if !is_link(link) || link.exists() {
        return None;
    }
    let target = fs::read_link(link).ok()?;
    // A relative link leads from the folder it is in.
    let target = match link.parent() {
        Some(dir) => dir.join(&target),
        None => target,
    };
    let parts: Vec<_> = target.components().collect();
    (1..parts.len()).find_map(|i| {
        let name = parts[i].as_os_str();
        if name != "bin" && name != "tmux" && name != "claude" {
            return None;
        }
        let clone: PathBuf = parts[..i].iter().collect();
        is_clone(&clone).then(|| (clone, parts[i..].iter().collect()))
    })
}

/// Why Claude Code still uses a link setup.sh made into `~/.claude`, or
/// `None` when nothing does: `commands/`, which Claude reads by itself,
/// and any link whose path a string in `~/.claude/settings.json` names
/// (a hook's command, `otelHeadersHelper`), as `~/`,
/// `$HOME/` or in full. The `hooks` folder as one link counts when a
/// string names anything in it. Permissions are left out, since allowing a
/// command doesn't run it. Settings that can't be read (`None`) may run any
/// link in `~/.claude`, so each counts.
fn still_used(link: &Path, settings: Option<&Value>) -> Option<&'static str> {
    let claude = paths::claude_dir();
    if link == claude.join("commands") {
        return Some("Claude Code reads it");
    }
    // The `claude` part moves a statusline running it onto `switchboard
    // statusline`, but only while the script isn't a file of yours, which a
    // copy put back would be.
    if link == claude.join("statusline-command.sh") {
        return None;
    }
    let Some(settings) = settings else {
        return link
            .starts_with(&claude)
            .then_some("~/.claude/settings.json can't be read, so it may run it");
    };
    let rest = link.strip_prefix(paths::home()).ok()?.to_string_lossy();
    let full = link.to_string_lossy();
    let mut names = [
        format!("~/{rest}"),
        format!("$HOME/{rest}"),
        format!("${{HOME}}/{rest}"),
        full.into_owned(),
    ];
    if link == claude.join("hooks") {
        names.iter_mut().for_each(|n| n.push('/'));
    }
    let mut strings = Vec::new();
    if let Value::Object(map) = settings {
        for (key, value) in map {
            if key != "permissions" {
                collect_strings(value, &mut strings);
            }
        }
    }
    let named = strings.iter().any(|s| {
        names.iter().any(|n| {
            s.match_indices(n.as_str()).any(|(at, _)| {
                // Not a longer name that starts the same.
                s[at + n.len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| n.ends_with('/') || !(c.is_alphanumeric() || "._-".contains(c)))
            })
        })
    });
    named.then_some("~/.claude/settings.json runs it")
}

fn collect_strings<'a>(value: &'a Value, out: &mut Vec<&'a str>) {
    match value {
        Value::String(s) => out.push(s),
        Value::Array(list) => list.iter().for_each(|v| collect_strings(v, out)),
        Value::Object(map) => map.values().for_each(|v| collect_strings(v, out)),
        _ => {}
    }
}

/// A file from a clone's history: its path below the link, whether it runs,
/// and what it holds.
type Held = (PathBuf, bool, Vec<u8>);

/// What `rel` (a file or a folder) held in `clone` before it was removed,
/// each file with its path below `rel`, or `None` when the clone's history
/// doesn't have it. Each file is taken from the commit before the last one
/// that removed it, since a folder's files may have gone in different
/// commits; a move counts as a removal (`--no-renames`), as the file is gone
/// from where the link led either way.
fn held_in_history(clone: &Path, rel: &Path) -> Option<Vec<Held>> {
    let rel_str = rel.to_str()?;
    let removed = worktree::git_at(
        clone,
        &[
            "log",
            "--no-renames",
            "--diff-filter=D",
            "--name-only",
            "--format=",
            "-z",
            "--",
            rel_str,
        ],
    )
    .ok()?;
    let mut files: Vec<&str> = removed
        .split(['\0', '\n'])
        .filter(|p| !p.is_empty())
        .collect();
    files.sort();
    files.dedup();
    let mut held = Vec::new();
    for file in files {
        // Only plain names below `rel`, so nothing lands outside the link.
        let below = Path::new(file).strip_prefix(rel).ok()?.to_path_buf();
        if !below
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
        {
            continue;
        }
        let commit = worktree::git_at(
            clone,
            &[
                "log",
                "-1",
                "--no-renames",
                "--diff-filter=D",
                "--format=%H",
                "--",
                file,
            ],
        )
        .ok()
        .filter(|h| !h.is_empty())?;
        let entry =
            worktree::git_at(clone, &["ls-tree", &format!("{commit}^"), "--", file]).ok()?;
        let (meta, _) = entry.split_once('\t')?;
        let mut meta = meta.split(' ');
        let (mode, kind, object) = (meta.next()?, meta.next()?, meta.next()?);
        if kind != "blob" || mode == "120000" {
            continue;
        }
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(clone)
            .args(["cat-file", "blob", object])
            .output()
            .ok()
            .filter(|o| o.status.success())?;
        held.push((below, mode == "100755", out.stdout));
    }
    (!held.is_empty()).then_some(held)
}

/// Puts what a link led to in its place, as files of its own.
fn put_back(link: &Path, held: &[Held]) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let shown = home_tilde(link);
    fs::remove_file(link).with_context(|| format!("can't remove {shown}"))?;
    for (below, runs, bytes) in held {
        let path = if below.as_os_str().is_empty() {
            link.to_path_buf()
        } else {
            link.join(below)
        };
        util::write_atomic(&path, bytes)
            .with_context(|| format!("can't write {}", home_tilde(&path)))?;
        let mode = if *runs { 0o755 } else { 0o644 };
        fs::set_permissions(&path, fs::Permissions::from_mode(mode))?;
    }
    Ok(())
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

/// Every link leading nowhere where setup.sh linked a program's config, and
/// where anyone following it would have: directly in `~` (`~/.pgclirc`), in
/// `~/.config` and one folder below it (`~/.config/ghostty/config`, the
/// folder link `~/.config/nvim`), in `~/Library/Application Support` and one
/// folder below it (lazygit's on a Mac; there is none on Linux), and directly
/// in `~/.claude` (`.mcp.json`), each folder looked through even when it is
/// a link to one (a dotfiles repository's `~/.config`). What another part
/// or `dead_links` handles is left out: `~/.config/tmux`, `~/.claude/skills`, Claude's settings and
/// CLAUDE.md, and setup.sh's single links in `~/.claude`.
fn config_links() -> Vec<PathBuf> {
    let claude = paths::claude_dir();
    let skipped: Vec<PathBuf> = [
        paths::home_join(".config/tmux"),
        claude.join("skills"),
        claude.join("settings.json"),
        claude.join("CLAUDE.md"),
    ]
    .into_iter()
    .chain(link_files())
    .collect();
    let entries = |dir: &Path| -> Vec<PathBuf> {
        fs::read_dir(dir)
            .map(|e| e.flatten().map(|e| e.path()).collect())
            .unwrap_or_default()
    };
    let mut found = Vec::new();
    let dead = |p: &Path| is_link(p) && !p.exists();
    let mut look = |dir: &Path, below: bool| {
        for path in entries(dir) {
            if skipped.contains(&path) {
                continue;
            }
            if dead(&path) {
                found.push(path);
            } else if below && path.is_dir() {
                found.extend(
                    entries(&path)
                        .into_iter()
                        .filter(|p| dead(p) && !skipped.contains(p)),
                );
            }
        }
    };
    look(&paths::home(), false);
    look(&paths::home_join(".config"), true);
    look(&paths::home_join("Library/Application Support"), true);
    look(&claude, false);
    found.sort();
    found.dedup();
    found
}

/// For a link leading nowhere into a clone, at any path in it: the clone,
/// the path it led to inside it (`karabiner/karabiner.json`), and the clone
/// whose history holds it, if any. That is the clone itself; for one gone
/// that was named `editor-configs`, the `switchboard` clone beside it, the
/// same repository renamed. A gone folder named `switchboard` isn't taken,
/// since `~/.config/switchboard` and the like share the name; a link
/// leading outside any clone is yours.
fn clone_of(link: &Path) -> Option<(PathBuf, PathBuf, Option<PathBuf>)> {
    if !is_link(link) || link.exists() {
        return None;
    }
    let target = fs::read_link(link).ok()?;
    // A relative link leads from the folder it is in.
    let target = match link.parent() {
        Some(dir) => dir.join(&target),
        None => target,
    };
    target.ancestors().skip(1).find_map(|dir| {
        let rel = target.strip_prefix(dir).ok()?.to_path_buf();
        if dir.exists() {
            return is_clone(dir).then(|| (dir.to_path_buf(), rel, Some(dir.to_path_buf())));
        }
        if dir.file_name()? != "editor-configs" {
            return None;
        }
        let renamed = dir.with_file_name("switchboard");
        let history = (renamed.exists() && is_clone(&renamed)).then_some(renamed);
        Some((dir.to_path_buf(), rel, history))
    })
}

/// What `rel` (a file or a folder) last held in `clone`: the tree under it
/// in the commit before the last one that removed anything under it, so a
/// folder comes back as it last was, without files removed long before.
/// `None` when the history never removed it. As `held_in_history`, a move
/// counts as a removal, only plain paths below `rel` are taken, links in
/// the history are passed over, and whether each file runs is kept.
fn held_last(clone: &Path, rel: &Path) -> Option<Vec<Held>> {
    let rel_str = rel.to_str()?;
    let commit = worktree::git_at(
        clone,
        &[
            "log",
            "-1",
            "--no-renames",
            "--diff-filter=D",
            "--format=%H",
            "--",
            rel_str,
        ],
    )
    .ok()
    .filter(|h| !h.is_empty())?;
    let tree = worktree::git_at(
        clone,
        &["ls-tree", "-r", "-z", &format!("{commit}^"), "--", rel_str],
    )
    .ok()?;
    let mut held = Vec::new();
    for entry in tree.split('\0').filter(|e| !e.is_empty()) {
        let (meta, file) = entry.split_once('\t')?;
        let mut meta = meta.split(' ');
        let (mode, kind, object) = (meta.next()?, meta.next()?, meta.next()?);
        if kind != "blob" || mode == "120000" {
            continue;
        }
        let Ok(below) = Path::new(file).strip_prefix(rel) else {
            continue;
        };
        if !below
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
        {
            continue;
        }
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(clone)
            .args(["cat-file", "blob", object])
            .output()
            .ok()
            .filter(|o| o.status.success())?;
        held.push((below.to_path_buf(), mode == "100755", out.stdout));
    }
    (!held.is_empty()).then_some(held)
}

/// The command the tmux version's Ghostty config started in every window,
/// `tmux new-session -A -s main` in a login shell, made to start the binary
/// in its place, `bin` (named in full, since a login shell that isn't
/// interactive skips `~/.zshrc`, where `~/.local/bin` is often put on the
/// PATH), or `None` when the config has no such line. Put back as it was,
/// every new window would open the retired tmux workspace, or fail without
/// tmux.
pub fn ghostty_unwrapped(text: &str, bin: &Path) -> Option<String> {
    let quoted = format!("'{}'", bin.to_string_lossy().replace('\'', "'\\''"));
    let line = Regex::new(r"^\s*command\s*=.*\btmux\b.*\bnew-session\b").expect("a valid pattern");
    let mut changed = false;
    let out: String = text
        .split_inclusive('\n')
        .map(|l| {
            if line.is_match(l) {
                changed = true;
                let end = if l.ends_with('\n') { "\n" } else { "" };
                format!("command = /bin/zsh -l -c {quoted}{end}")
            } else {
                l.to_string()
            }
        })
        .collect();
    changed.then_some(out)
}

/// Whether `path` is a Ghostty config: a file in `~/.config/ghostty` or a
/// Mac's `com.mitchellh.ghostty`.
fn is_ghostty_config(path: &Path) -> bool {
    path.parent()
        .and_then(|d| d.file_name())
        .is_some_and(|n| n == "ghostty" || n == "com.mitchellh.ghostty")
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
    let settings = claude_settings::as_claude_reads();
    let mut left = Vec::new();
    for link in dead_links() {
        if let Some(why) = still_used(&link, settings.as_ref()) {
            let shown = home_tilde(&link);
            let Some((clone, rel)) = clone_target(&link) else {
                continue;
            };
            let Some(held) = held_in_history(&clone, &rel) else {
                left.push(format!(
                    "{shown} leads to {}, which isn't there and can't be found in its clone's history, and {why}; left alone",
                    clone.join(&rel).display()
                ));
                continue;
            };
            if !r.check {
                put_back(&link, &held)?;
            }
            r.change(format!(
                "replace {shown}, which led to what the tmux version had, with a copy of it from {}'s history, since {why}",
                home_tilde(&clone)
            ));
            continue;
        }
        if !r.check {
            fs::remove_file(&link)
                .with_context(|| format!("can't remove {}", home_tilde(&link)))?;
        }
        r.change(format!(
            "remove {}, which led to a file the tmux version had",
            home_tilde(&link)
        ));
    }
    for link in config_links() {
        let Some((clone, rel, history)) = clone_of(&link) else {
            continue;
        };
        let shown = home_tilde(&link);
        let found = history
            .as_ref()
            .and_then(|h| held_last(h, &rel).map(|held| (h, held)));
        let Some((history, held)) = found else {
            // In a clone still there, a path its history never removed (a
            // build's output) may come back, so only the older name's go,
            // and of those only one whose path the renamed clone doesn't
            // still have.
            let still_there = history.as_ref().is_some_and(|h| h.join(&rel).exists());
            if clone.exists() || still_there {
                continue;
            }
            if !r.check {
                fs::remove_file(&link).with_context(|| format!("can't remove {shown}"))?;
            }
            r.change(format!(
                "remove {shown}, which led to {}, which no history has",
                home_tilde(&clone.join(&rel))
            ));
            continue;
        };
        let mut held = held;
        let bin = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("switchboard"));
        let mut unwrapped = false;
        for (below, _, bytes) in &mut held {
            let path = if below.as_os_str().is_empty() {
                link.clone()
            } else {
                link.join(&*below)
            };
            let fixed = is_ghostty_config(&path)
                .then(|| {
                    std::str::from_utf8(bytes)
                        .ok()
                        .and_then(|t| ghostty_unwrapped(t, &bin))
                })
                .flatten();
            if let Some(fixed) = fixed {
                *bytes = fixed.into_bytes();
                unwrapped = true;
            }
        }
        if !r.check {
            put_back(&link, &held)?;
        }
        r.change(format!(
            "replace {shown}, which led into {}, with its copy from {}'s history{}",
            home_tilde(&clone),
            home_tilde(history),
            if unwrapped {
                ", its command starting switchboard in place of tmux"
            } else {
                ""
            }
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
    if !left.is_empty() {
        anyhow::bail!("{}", left.join("\n"));
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
    fn ghostty_starts_switchboard_in_place_of_tmux() {
        let text = "font-size = 14\n\
                    command = /bin/zsh -l -c '/opt/homebrew/bin/tmux new-session -A -s main'\n\
                    keybind = cmd+t=new_tab\n";
        assert_eq!(
            ghostty_unwrapped(text, Path::new("/Users/me/.local/bin/switchboard")).unwrap(),
            "font-size = 14\n\
             command = /bin/zsh -l -c '/Users/me/.local/bin/switchboard'\n\
             keybind = cmd+t=new_tab\n"
        );
        assert_eq!(
            ghostty_unwrapped("command = /bin/zsh -l\n", Path::new("x")),
            None
        );
        assert_eq!(
            ghostty_unwrapped("# tmux new-session\n", Path::new("x")),
            None
        );
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
