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
//! the plugin's too, so the part removes each one Switchboard put there:
//! the files its list names, the list, and what the skill installed for
//! itself beside them (`npm ci`'s `node_modules`), and the folder once
//! nothing else is in it. A file you added there is yours, so the folder is
//! left with it, which the part says; without a SKILL.md it is no skill
//! Claude Code runs, and the old way writes Switchboard's into it again
//! beside your files.
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

use std::collections::{BTreeMap, BTreeSet};
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

/// Every skill Switchboard ships, by name, each with its files (their
/// paths within the skill, and their bytes): the clone's, else what the
/// binary carries (src/assets.rs).
pub fn shipped() -> BTreeMap<String, Vec<(String, Vec<u8>)>> {
    let mut skills: BTreeMap<String, Vec<(String, Vec<u8>)>> = BTreeMap::new();
    for (path, bytes) in crate::assets::skills() {
        if let Some((name, rest)) = path.split_once('/') {
            skills
                .entry(name.to_string())
                .or_default()
                .push((rest.to_string(), bytes));
        }
    }
    skills
}

/// One skill's file, as `shipped` has it: `"wt/SKILL.md"`.
pub fn shipped_file(path: &str) -> Option<Vec<u8>> {
    crate::assets::skills()
        .into_iter()
        .find(|(p, _)| p == path)
        .map(|(_, b)| b)
}

/// What is at `~/.claude/skills/<name>`, as far as this part cares.
#[derive(Debug, PartialEq, Eq)]
enum Found {
    Nothing,
    /// A folder Switchboard wrote, with the files its list names.
    Ours(Vec<String>),
    /// A link into a Switchboard clone's skills, as setup.sh made.
    Linked,
    /// A folder with neither the list nor a SKILL.md: what was left of one
    /// of Switchboard's taken out with files of yours in it, which isn't a
    /// skill. The old way writes Switchboard's into it, beside them.
    Left,
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
        _ if dest.is_dir() && !exists(&dest.join("SKILL.md")) => Found::Left,
        _ => Found::Own,
    }
}

/// Whether a path from a skill's list stays inside its folder.
fn inside(rel: &str) -> bool {
    !rel.split('/')
        .any(|p| p == ".." || p == "." || p.is_empty())
}

/// The folders within a skill's folder that hold a file its list names,
/// where the packages a skill installs for itself go (`npm ci`'s
/// `node_modules` beside its `package.json`): `""` for the folder itself.
fn listed_dirs(list: &[String]) -> Vec<String> {
    let mut dirs: Vec<String> = list
        .iter()
        .filter(|rel| inside(rel))
        .map(|rel| rel.rsplit_once('/').map_or("", |(dir, _)| dir).to_string())
        .collect();
    dirs.sort();
    dirs.dedup();
    dirs
}

/// What is in a skill folder Switchboard wrote that taking it out leaves:
/// every file but the ones its list names, the list itself, and the
/// packages beside them. Their paths within it, in order.
fn yours_in(dest: &Path, list: &[String]) -> Vec<String> {
    let packages: Vec<String> = listed_dirs(list)
        .iter()
        .map(|d| {
            if d.is_empty() {
                "node_modules".to_string()
            } else {
                format!("{d}/node_modules")
            }
        })
        .collect();
    let mut yours = Vec::new();
    let mut stack = vec![String::new()];
    while let Some(rel_dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(dest.join(&rel_dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = if rel_dir.is_empty() {
                name
            } else {
                format!("{rel_dir}/{name}")
            };
            // A link is a file here, never followed.
            let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
            if is_dir {
                if !packages.contains(&rel) {
                    stack.push(rel);
                }
            } else if rel != MANIFEST && !list.contains(&rel) {
                yours.push(rel);
            }
        }
    }
    yours.sort();
    yours
}

/// A skill folder Switchboard wrote taken out of `~/.claude/skills`: the
/// files its list names, the packages beside them and the list, then every
/// folder that leaves empty, the skill's own too. What is left is yours
/// (`yours_in`), kept in the folder.
fn take_out(dest: &Path, list: &[String]) -> Result<()> {
    for rel in list.iter().filter(|rel| inside(rel)) {
        let file = dest.join(rel);
        if file.is_dir() && !is_link(&file) {
            continue;
        }
        match fs::remove_file(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
    }
    let dirs = listed_dirs(list);
    for dir in &dirs {
        let modules = dest.join(dir).join("node_modules");
        if is_link(&modules) {
            fs::remove_file(&modules)?;
        } else if modules.is_dir() {
            fs::remove_dir_all(&modules)?;
        }
    }
    match fs::remove_file(dest.join(MANIFEST)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
        _ => {}
    }
    // The folders left empty, deepest first, up to the skill's own.
    let mut empty: BTreeSet<PathBuf> = BTreeSet::new();
    for dir in dirs.iter().filter(|d| !d.is_empty()) {
        let mut at = dest.join(dir);
        while at != dest && at.starts_with(dest) {
            empty.insert(at.clone());
            match at.parent() {
                Some(p) => at = p.to_path_buf(),
                None => break,
            }
        }
    }
    let mut empty: Vec<PathBuf> = empty.into_iter().collect();
    empty.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    empty.push(dest.to_path_buf());
    for dir in &empty {
        // One still holding something of yours stays.
        let _ = fs::remove_dir(dir);
    }
    Ok(())
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
/// written there, or linked from a clone, or a skill folder Switchboard
/// wrote that it no longer ships, which `run` takes out.
pub fn installed() -> bool {
    let names = fs::read_dir(skills_dir()).into_iter().flatten().flatten();
    let wrote_one = names
        .filter_map(|e| e.file_name().into_string().ok())
        .any(|name| matches!(found(&name), Found::Ours(_)));
    wrote_one
        || shipped()
            .keys()
            .any(|name| matches!(found(name), Found::Linked))
}

/// Whether `~/.claude/skills/<name>` is Switchboard's skill by that name,
/// for `skill-path` to follow: one the old way wrote there (its folder holds
/// `MANIFEST`) or a link into a clone's, or a copy of it made yours as the
/// guide says to change one (its `MANIFEST` deleted), known by its opening
/// still running `skill-path <name>`. A skill of yours that only shares the
/// name isn't, so Switchboard's own (the plugin's) is followed in its place.
pub fn switchboard_s(name: &str) -> bool {
    match found(name) {
        Found::Ours(_) | Found::Linked => true,
        Found::Own => fs::read_to_string(skills_dir().join(name).join("SKILL.md"))
            .is_ok_and(|text| text.contains(&format!("skill-path {name}`"))),
        Found::Nothing | Found::Left => false,
    }
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
    // The folders taken out but for files of yours, with those files.
    let mut left: Vec<(PathBuf, Vec<String>)> = Vec::new();
    if way == Way::Old && !r.check {
        fs::create_dir_all(&dir)?;
    }
    let shipped = shipped();
    // Either way, a skill Switchboard wrote that it ships no longer (one
    // removed or renamed, or the clone's own once the build went back to
    // what it carries, src/assets.rs) is taken out, keeping files of yours.
    let mut retired = Vec::new();
    {
        let names = fs::read_dir(&dir).into_iter().flatten().flatten();
        for name in names.filter_map(|e| e.file_name().into_string().ok()) {
            if shipped.contains_key(&name) {
                continue;
            }
            if let Found::Ours(list) = found(&name) {
                let dest = dir.join(&name);
                let yours = yours_in(&dest, &list);
                if !yours.is_empty() {
                    left.push((dest.clone(), yours));
                }
                if r.check || take_out(&dest, &list).is_ok() {
                    retired.push(name);
                } else {
                    failed.push(name);
                }
            }
        }
    }
    if !retired.is_empty() {
        r.change(format!(
            "take the skills Switchboard no longer ships out of {} ({})",
            home_tilde(&dir),
            retired.join(" ")
        ));
    }
    for (name, files) in shipped {
        let dest = dir.join(&name);
        let result = match way {
            Way::Old => write_skill(r, &dest, &name, &files),
            Way::Plugin => match found(&name) {
                Found::Ours(list) => {
                    let yours = yours_in(&dest, &list);
                    if !yours.is_empty() {
                        left.push((dest.clone(), yours));
                    }
                    if r.check {
                        Ok(())
                    } else {
                        take_out(&dest, &list)
                    }
                }
                Found::Linked if !r.check => fs::remove_file(&dest).map_err(Into::into),
                Found::Linked => Ok(()),
                Found::Nothing | Found::Left | Found::Own => continue,
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
    for (folder, yours) in left {
        r.change(format!(
            "leave {} for the files in it Switchboard didn't write ({}): without its SKILL.md, Claude Code takes it for no skill",
            home_tilde(&folder),
            yours.join(", ")
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
fn write_skill(r: &mut Report, dest: &Path, name: &str, files: &[(String, Vec<u8>)]) -> Result<()> {
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
        Found::Left => {
            r.change(format!("write {shown}, beside the files of yours in it"));
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

    #[test]
    fn taking_a_skill_out_removes_only_what_switchboard_wrote_and_its_packages() {
        let dest = std::env::temp_dir().join(format!("sb-skill-out-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dest);
        let write = |rel: &str| {
            let file = dest.join(rel);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, "x").unwrap();
        };
        let list: Vec<String> = ["SKILL.md", "scripts/record.js", "scripts/package.json"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        for rel in &list {
            write(rel);
        }
        write(MANIFEST);
        write("scripts/node_modules/x/index.js");

        // Only Switchboard's: the folder goes whole.
        assert!(yours_in(&dest, &list).is_empty());
        take_out(&dest, &list).unwrap();
        assert!(!dest.exists());

        // With files of yours: they stay, the folder with them.
        for rel in &list {
            write(rel);
        }
        write(MANIFEST);
        write("scripts/node_modules/x/index.js");
        write("notes.md");
        write("mine/plan.md");
        assert_eq!(yours_in(&dest, &list), ["mine/plan.md", "notes.md"]);
        take_out(&dest, &list).unwrap();
        assert!(dest.join("notes.md").is_file());
        assert!(dest.join("mine/plan.md").is_file());
        assert!(!dest.join("SKILL.md").exists());
        assert!(!dest.join(MANIFEST).exists());
        assert!(!dest.join("scripts").exists(), "left empty, so gone");
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn a_list_s_path_outside_its_folder_is_never_followed() {
        assert!(inside("scripts/record.js"));
        assert!(!inside("../other/SKILL.md"));
        assert!(!inside("a//b"));
        assert!(!inside("./x"));
        assert_eq!(
            listed_dirs(&["SKILL.md".into(), "scripts/a.js".into(), "../x".into()]),
            ["", "scripts"]
        );
    }
}
