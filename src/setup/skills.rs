//! The skills that ship with Switchboard (/wt, /shape, /review-loop,
//! /ci-loop and the rest), as the binary carries them (`assets/skills`,
//! compiled in by build.rs), and the `skills` part, which puts them in
//! `~/.claude/skills` or takes them out of it, as the way setup puts
//! Switchboard says (src/setup/way.rs). Ported from setup.sh's
//! `part_skills`, which linked each from a clone's `claude/skills`; the
//! binary carries them instead.
//!
//! As a plugin, the skills are the plugin's (src/setup/plugin.rs), loaded
//! only by the Claude sessions in Switchboard's panes, so Claude elsewhere
//! has your own skills and none of Switchboard's. A copy left in
//! `~/.claude/skills` would take the skill's bare name (`/wt`) everywhere,
//! the plugin's too, so the part removes each one Switchboard put there,
//! with whatever the skill made for itself beside its files (`npm ci`'s
//! `node_modules`).
//!
//! The old way, each is written into `~/.claude/skills`, where Claude Code
//! finds a personal skill, as a folder of its own, written out again
//! whenever what the binary carries differs from it. A skill folder
//! Switchboard wrote holds `.switchboard-skill`, the list of the files it
//! wrote, so a later run knows the folder is Switchboard's to bring up to
//! date, removes a file a newer version no longer ships, and leaves alone
//! what the skill made for itself.
//!
//! Either way, a skill of your own by the same name, a folder without that
//! list or a link elsewhere, is yours and kept as it is (as a plugin, Claude
//! Code runs it for the bare name, and Switchboard's as
//! `/switchboard:<name>`); a link into a Switchboard clone's skills, as
//! setup.sh made, is Switchboard's.
//!
//! Beside them, `skill-path` and `codex-review` are linked into
//! `~/.local/bin` to the binary (`link_commands`), as setup.sh linked its
//! scripts there, since a repository's own copy of a skill, written for the
//! tmux version, runs them by those names.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use super::way::Way;
use super::{Report, exists, home_tilde, is_link};
use crate::paths;

include!(concat!(env!("OUT_DIR"), "/skills.rs"));

/// The list of what an earlier version wrote, in each skill folder of its
/// own.
pub const MANIFEST: &str = ".switchboard-skill";

/// `~/.claude/skills`.
pub fn skills_dir() -> PathBuf {
    paths::claude_dir().join("skills")
}

/// Every skill the binary carries, by name, each with its files (their
/// paths within the skill, and their bytes).
pub fn shipped() -> BTreeMap<&'static str, Vec<(&'static str, &'static [u8])>> {
    let mut skills: BTreeMap<&str, Vec<(&str, &[u8])>> = BTreeMap::new();
    for (path, bytes) in FILES {
        if let Some((name, rest)) = path.split_once('/') {
            skills.entry(name).or_default().push((rest, bytes));
        }
    }
    skills
}

/// One skill's file, as the binary carries it: `"wt/SKILL.md"`.
pub fn shipped_file(path: &str) -> Option<&'static [u8]> {
    FILES.iter().find(|(p, _)| *p == path).map(|(_, b)| *b)
}

/// What is at `~/.claude/skills/<name>`, as far as this part cares.
#[derive(Debug, PartialEq, Eq)]
enum Found {
    Nothing,
    /// A folder Switchboard wrote, with the files its list names.
    Ours(Vec<String>),
    /// A link into a Switchboard clone's skills, as setup.sh made.
    Linked,
    /// Yours: kept.
    Own,
}

fn found(name: &str) -> Found {
    let dest = skills_dir().join(name);
    if !exists(&dest) {
        return Found::Nothing;
    }
    if is_link(&dest) {
        return if from_switchboard(name) {
            Found::Linked
        } else {
            Found::Own
        };
    }
    match fs::read_to_string(dest.join(MANIFEST)) {
        Ok(list) if dest.is_dir() => Found::Ours(
            list.lines()
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect(),
        ),
        _ => Found::Own,
    }
}

/// Whether `~/.claude/skills/<name>` links into a Switchboard clone, known by
/// what is in it, whatever its folder is called: one gone (a clone moved or
/// renamed) counts too, since the link leads nowhere. setup.sh's
/// `from_switchboard`, which knew a clone by its setup.sh, tmux.conf and
/// sidebar; a clone of the Rust version is known by its Cargo.toml and
/// skills.
fn from_switchboard(name: &str) -> bool {
    let link = skills_dir().join(name);
    let Ok(target) = fs::read_link(&link) else {
        return false;
    };
    // Written by hand, it may end in a /.
    let text = target.to_string_lossy();
    let text = text.trim_end_matches('/');
    let clone = [
        format!("/claude/skills/{name}"),
        format!("/assets/skills/{name}"),
    ]
    .iter()
    .find_map(|tail| text.strip_suffix(tail.as_str()).map(str::to_string));
    let Some(clone) = clone else {
        return false;
    };
    // A relative link leads from the folder it is in.
    let base = |p: &str| {
        let p = PathBuf::from(p);
        if p.is_absolute() {
            p
        } else {
            skills_dir().join(p)
        }
    };
    let (target, clone) = (base(text), base(&clone));
    !target.exists()
        || (clone.join("setup.sh").is_file()
            && clone.join("tmux/tmux.conf").is_file()
            && clone.join("lib/sidebar").is_dir())
        || (clone.join("Cargo.toml").is_file() && clone.join("assets/skills").is_dir())
}

/// Whether Switchboard's skills are in `~/.claude/skills`: any of them
/// written there, or linked from a clone.
pub fn installed() -> bool {
    shipped()
        .keys()
        .any(|name| matches!(found(name), Found::Ours(_) | Found::Linked))
}

/// The skills of your own kept in place of Switchboard's of the same name.
pub fn own_skills() -> Vec<String> {
    shipped()
        .keys()
        .filter(|name| found(name) == Found::Own)
        .map(|name| name.to_string())
        .collect()
}

/// The part, the way setup puts Switchboard: each skill written or brought
/// up to date in `~/.claude/skills` the old way, taken out of it as a
/// plugin; every one tried and the ones that couldn't be said together at
/// the end; and the commands linked.
pub fn run(r: &mut Report) -> Result<()> {
    let dir = skills_dir();
    let way = super::way::current();
    let mut done = Vec::new();
    let mut failed = Vec::new();
    if way == Way::Old && !r.check {
        fs::create_dir_all(&dir)?;
    }
    for (name, files) in shipped() {
        let dest = dir.join(name);
        let result = match way {
            Way::Old => write_skill(r, &dest, name, &files),
            Way::Plugin => match found(name) {
                Found::Ours(_) if !r.check => fs::remove_dir_all(&dest).map_err(Into::into),
                Found::Linked if !r.check => fs::remove_file(&dest).map_err(Into::into),
                Found::Ours(_) | Found::Linked => Ok(()),
                Found::Nothing | Found::Own => continue,
            },
        };
        match result {
            Ok(()) => done.push(name),
            Err(_) => failed.push(name),
        }
    }
    if way == Way::Plugin && !done.is_empty() {
        r.change(format!(
            "take Switchboard's skills out of {} ({}): its plugin carries them for the Claude in its panes",
            home_tilde(&dir),
            done.join(" ")
        ));
    }
    if let Err(e) = link_commands(r) {
        bail!("could not link the commands skills run into ~/.local/bin: {e}");
    }
    if failed.is_empty() {
        return Ok(());
    }
    bail!(
        "could not {} these skills {} {}: {}",
        if way == Way::Old { "write" } else { "take" },
        if way == Way::Old { "into" } else { "out of" },
        home_tilde(&dir),
        failed.join(" ")
    )
}

/// The commands a repository's own copy of a skill, written for the tmux
/// version, runs by their bare names: setup.sh linked them into
/// `~/.local/bin`, and the binary runs the command a link is named after.
const COMMAND_LINKS: [&str; 2] = ["skill-path", "codex-review"];

/// Each of `COMMAND_LINKS` in `~/.local/bin` as a link to this binary: made
/// where nothing is, and in place of a link that leads nowhere (one into a
/// clone of the tmux version, whose script is gone). A file there, or a link
/// to something that is there, is yours and left alone.
fn link_commands(r: &mut Report) -> Result<()> {
    let exe = crate::util::exe()?;
    let exe = fs::canonicalize(&exe).unwrap_or(exe);
    let bin = paths::home_join(".local/bin");
    for name in COMMAND_LINKS {
        let link = bin.join(name);
        let shown = home_tilde(&link);
        if fs::canonicalize(&link).is_ok_and(|to| to == exe) {
            continue;
        }
        if exists(&link) {
            if !is_link(&link) || link.exists() {
                continue;
            }
            r.change(format!(
                "replace {shown}, a link leading nowhere, with one to switchboard"
            ));
            if !r.check {
                fs::remove_file(&link)?;
            }
        } else {
            r.change(format!("link {shown} to switchboard"));
        }
        if !r.check {
            fs::create_dir_all(&bin)?;
            std::os::unix::fs::symlink(&exe, &link)?;
        }
    }
    Ok(())
}

/// One skill written into `~/.claude/skills`, the old way.
fn write_skill(r: &mut Report, dest: &Path, name: &str, files: &[(&str, &[u8])]) -> Result<()> {
    let shown = home_tilde(dest);
    // A skill new here, or a link replaced, is said in one line; one
    // already written, by what changed in it.
    let (before, said) = match found(name) {
        Found::Own => return Ok(()),
        Found::Linked => {
            if !r.check {
                fs::remove_file(dest)?;
            }
            r.change(format!("replace the link {shown} with Switchboard's skill"));
            (Vec::new(), true)
        }
        Found::Nothing => {
            r.change(format!("write {shown}"));
            (Vec::new(), true)
        }
        Found::Ours(list) => (list, false),
    };
    let mut written = 0;
    for (rel, bytes) in files {
        let file = dest.join(rel);
        if fs::read(&file).is_ok_and(|now| now == *bytes) {
            continue;
        }
        if !r.check {
            if let Some(parent) = file.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&file, bytes)?;
        }
        written += 1;
    }
    let mut gone = 0;
    for rel in &before {
        if files.iter().any(|(f, _)| f == rel) || rel.split('/').any(|p| p == ".." || p.is_empty())
        {
            continue;
        }
        let file = dest.join(rel);
        if file.is_file() {
            if !r.check {
                fs::remove_file(&file)?;
                // The folders it leaves empty go with it.
                let mut parent = file.parent();
                while let Some(p) = parent.filter(|p| *p != dest) {
                    if fs::remove_dir(p).is_err() {
                        break;
                    }
                    parent = p.parent();
                }
            }
            gone += 1;
        }
    }
    let list: String = files.iter().map(|(f, _)| format!("{f}\n")).collect();
    let list_changed = fs::read_to_string(dest.join(MANIFEST)).ok().as_deref() != Some(&list);
    if list_changed && !r.check {
        fs::create_dir_all(dest)?;
        fs::write(dest.join(MANIFEST), &list)?;
    }
    if said {
        return Ok(());
    }
    if written + gone > 0 {
        let mut what = Vec::new();
        if written > 0 {
            what.push(format!(
                "{written} file{} written",
                if written == 1 { "" } else { "s" }
            ));
        }
        if gone > 0 {
            what.push(format!("{gone} no longer shipped removed"));
        }
        r.change(format!("bring {shown} up to date ({})", what.join(", ")));
    } else if list_changed {
        r.change(format!("mark {shown} as Switchboard's"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_binary_carries_every_skill_with_its_skill_md() {
        let skills = shipped();
        assert!(skills.len() >= 15, "{:?}", skills.keys());
        for (name, files) in &skills {
            assert!(
                files.iter().any(|(f, _)| *f == "SKILL.md"),
                "{name} has a SKILL.md"
            );
            assert!(
                !files.iter().any(|(f, _)| f.contains("node_modules")),
                "{name} ships no packages"
            );
        }
        assert!(shipped_file("note-watch/pick-note.md").is_some());
        assert!(shipped_file("walkthrough/scripts/record.js").is_some());
    }
}
