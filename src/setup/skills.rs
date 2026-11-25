//! The skills that ship with Switchboard (/wt, /shape, /review-loop,
//! /ci-loop and the rest), as the binary carries them (`assets/skills`,
//! compiled in by build.rs), which its plugin holds (src/setup/plugin.rs),
//! loaded only by the Claude sessions in Switchboard's panes, so Claude
//! elsewhere has your own skills and none of Switchboard's.
//!
//! The `skills` part takes out the copies earlier versions wrote into
//! `~/.claude/skills`, which would take each skill's bare name (`/wt`)
//! everywhere, the plugin's too. A skill folder Switchboard wrote holds
//! `.switchboard-skill`, the list of the files it wrote: the part removes
//! those files, the list, and what the skill installed for itself beside
//! them (`npm ci`'s `node_modules`), and the folder once nothing else is
//! in it. A file you added there is yours, so the folder is left with it,
//! which the part says; without a SKILL.md it is no skill Claude Code runs.
//! A skill of your own by the same name, a folder without that list or a
//! link, is yours and kept as it is: Claude Code runs it for the bare name,
//! and Switchboard's as `/switchboard:<name>`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

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
    /// A folder with neither the list nor a SKILL.md: what was left of one
    /// of Switchboard's taken out with files of yours in it, which isn't a
    /// skill.
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
        return Found::Own;
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

/// The skill folders earlier versions wrote into `~/.claude/skills`, by
/// name, each with the files its list names.
fn written() -> Vec<(String, Vec<String>)> {
    let names = fs::read_dir(skills_dir()).into_iter().flatten().flatten();
    names
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|name| match found(&name) {
            Found::Ours(list) => Some((name, list)),
            _ => None,
        })
        .collect()
}

/// Whether an earlier version's skills are in `~/.claude/skills`.
pub fn installed() -> bool {
    !written().is_empty()
}

/// Whether `~/.claude/skills/<name>` is Switchboard's skill by that name,
/// for `skill-path` to follow: one an earlier version wrote there (its
/// folder holds `MANIFEST`), or a copy of it made yours as the guide says
/// to change one (its `MANIFEST` deleted), known by its opening still
/// running `skill-path <name>`. A skill of yours that only shares the name
/// isn't, so Switchboard's own (the plugin's) is followed in its place.
pub fn switchboard_s(name: &str) -> bool {
    match found(name) {
        Found::Ours(_) => true,
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

/// The part: every skill an earlier version wrote taken out of
/// `~/.claude/skills`, each tried and the ones that couldn't be said
/// together at the end.
pub fn run(r: &mut Report) -> Result<()> {
    let dir = skills_dir();
    let mut done = Vec::new();
    let mut failed = Vec::new();
    // The folders taken out but for files of yours, with those files.
    let mut left: Vec<(PathBuf, Vec<String>)> = Vec::new();
    for (name, list) in written() {
        let dest = dir.join(&name);
        let yours = yours_in(&dest, &list);
        if !yours.is_empty() {
            left.push((dest.clone(), yours));
        }
        if r.check || take_out(&dest, &list).is_ok() {
            done.push(name);
        } else {
            failed.push(name);
        }
    }
    if !done.is_empty() {
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
    if failed.is_empty() {
        return Ok(());
    }
    bail!(
        "could not take these skills out of {}: {}",
        home_tilde(&dir),
        failed.join(" ")
    )
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
