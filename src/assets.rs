//! What the binary carries in `assets/` (the skills, Claude's settings,
//! Switchboard's part of CLAUDE.md, the settings list), read from the clone
//! the build came from while that clone is there, so a skill or an
//! instruction landed on main is live without a rebuild, an install and a
//! restart. A release build has no clone (build.rs records none), and a
//! binary whose clone is gone, or one whose clone holds someone else's PR
//! (src/trust.rs), keeps what it carries; so does every build with Settings'
//! `assets_from_checkout` off.
//!
//! The clone is the checkout the build ran in, else the main clone it
//! belongs to (a worktree is removed once its work lands), as build.rs
//! records them; `SWITCHBOARD_CHECKOUT` in the environment wins, empty
//! meaning none (a test's server, which reads only what its binary
//! carries), as `switchboard try` names the worktree it built. Which one is
//! decided on each read, so a server whose worktree was removed once its
//! work landed goes on to the main clone, with no git run (a few stats and
//! two small files), since `switchboard hook` reads the settings on every
//! tool call; a file is read again only when its size or modified time
//! changes.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use serde_json::Value;

use crate::{settings, skill_files, util};

/// The setting that turns it off.
pub const SETTING: &str = "assets_from_checkout";

/// The clone's `assets/` folder, as the module's comment says, or `None`.
pub fn dir() -> Option<PathBuf> {
    if !enabled() {
        return None;
    }
    let given = std::env::var("SWITCHBOARD_CHECKOUT").ok();
    let given_main = std::env::var("SWITCHBOARD_MAIN_CLONE").ok();
    clone_assets(
        given.as_deref(),
        given_main.as_deref(),
        env!("SWITCHBOARD_CHECKOUT"),
        env!("SWITCHBOARD_MAIN_CLONE"),
    )
}

/// Whether the setting is on, read from your file itself: the settings list
/// this decides may be the clone's, so `settings::is_on` can't be asked.
fn enabled() -> bool {
    util::read_json_object(settings::settings_file()).get(SETTING) != Some(&Value::Bool(false))
}

/// `dir`'s choice: the environment's checkout when it gives one (empty for
/// none), else its main clone, else the built-in checkout, else its main
/// clone; the first whose `assets/` is there, and only when it holds your
/// code.
fn clone_assets(
    given: Option<&str>,
    given_main: Option<&str>,
    built_in: &str,
    main: &str,
) -> Option<PathBuf> {
    let nonempty = |s: &str| (!s.trim().is_empty()).then(|| PathBuf::from(s.trim()));
    let candidates: Vec<PathBuf> = match (given, given_main.and_then(nonempty)) {
        (Some(given), _) => nonempty(given).into_iter().collect(),
        (None, Some(main)) => vec![main],
        (None, None) => [built_in, main].into_iter().filter_map(nonempty).collect(),
    };
    candidates
        .into_iter()
        .find(|clone| clone.join("assets").is_dir())
        .filter(|clone| !crate::trust::is_untrusted_without_git(clone))
        .map(|clone| clone.join("assets"))
}

/// `<dir>/<name>` as it is on disk, else `compiled`.
pub fn text(name: &str, compiled: &'static str) -> Cow<'static, str> {
    match dir().and_then(|dir| read_cached(&dir.join(name))) {
        Some(text) => Cow::Owned(text),
        None => Cow::Borrowed(compiled),
    }
}

/// A file's text, read again only when its size or modified time changed.
fn read_cached(file: &Path) -> Option<String> {
    type Seen = (u64, Option<SystemTime>, String);
    static CACHE: Mutex<Option<HashMap<PathBuf, Seen>>> = Mutex::new(None);
    let meta = std::fs::metadata(file).ok()?;
    let stamp = (meta.len(), meta.modified().ok());
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let cache = cache.get_or_insert_with(HashMap::new);
    if let Some((len, modified, text)) = cache.get(file)
        && (*len, *modified) == stamp
    {
        return Some(text.clone());
    }
    let text = std::fs::read_to_string(file).ok()?;
    cache.insert(file.to_path_buf(), (stamp.0, stamp.1, text.clone()));
    Some(text)
}

/// A JSON asset parsed, or the compiled one when the clone's doesn't parse
/// (a half-saved edit), which is said once a process.
pub fn json(name: &str, compiled: &'static str) -> Value {
    static SAID: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let text = text(name, compiled);
    match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(e) => {
            let mut said = SAID.lock().unwrap_or_else(|e| e.into_inner());
            if !said.iter().any(|n| n == name) {
                said.push(name.to_string());
                eprintln!(
                    "switchboard: the clone's assets/{name} doesn't parse ({e}), so the build's own is used"
                );
            }
            serde_json::from_str(compiled).expect("the compiled asset parses")
        }
    }
}

/// Every file of every skill, by its path under `assets/skills/`, with its
/// bytes: the clone's, else what the binary carries.
pub fn skills() -> Vec<(String, Vec<u8>)> {
    match dir().map(|dir| dir.join("skills")).filter(|d| d.is_dir()) {
        Some(root) => skills_in(&root),
        None => crate::setup::skills::FILES
            .iter()
            .map(|(path, bytes)| (path.to_string(), bytes.to_vec()))
            .collect(),
    }
}

/// The skills under `root`, as build.rs walks them.
fn skills_in(root: &Path) -> Vec<(String, Vec<u8>)> {
    skill_files::under(root)
        .into_iter()
        .filter_map(|file| {
            let rel = skill_files::relative(root, &file)?;
            Some((rel, std::fs::read(&file).ok()?))
        })
        .collect()
}

/// What the parts are made from in `dir` (the skills, Claude's settings
/// and Switchboard's part of CLAUDE.md) as one hash of each file's path,
/// size and modified time, for the server's look: a file added, removed,
/// renamed or changed moves it.
pub fn fingerprint(dir: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    let files = skill_files::under(&dir.join("skills"))
        .into_iter()
        .chain([dir.join("claude-settings.json"), dir.join("switchboard.md")]);
    for file in files {
        file.hash(&mut hash);
        if let Ok(meta) = std::fs::metadata(&file) {
            meta.len().hash(&mut hash);
            meta.modified().ok().hash(&mut hash);
        }
    }
    hash.finish()
}

/// Where the assets come from, in words: "from ~/switchboard/assets", or
/// "as the binary carries them".
pub fn source_words() -> String {
    match dir() {
        Some(dir) => format!("from {}", crate::paths::tilde(&dir)),
        None => "as the binary carries them".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clone_with_assets(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sb-assets-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        dir
    }

    #[test]
    fn a_release_build_or_a_gone_clone_reads_none() {
        assert_eq!(clone_assets(None, None, "", ""), None);
        assert_eq!(clone_assets(None, None, "/nonexistent/sb", ""), None);
        assert_eq!(clone_assets(Some(""), None, "/nonexistent/sb", ""), None);
    }

    #[test]
    fn the_checkout_comes_before_its_main_clone_and_the_environment_before_both() {
        let built = clone_with_assets("built");
        let main = clone_with_assets("main");
        let given = clone_with_assets("given");
        let (b, m, g) = (
            built.to_str().unwrap(),
            main.to_str().unwrap(),
            given.to_str().unwrap(),
        );
        assert_eq!(clone_assets(None, None, b, m), Some(built.join("assets")));
        assert_eq!(
            clone_assets(None, None, "/nonexistent/wt", m),
            Some(main.join("assets"))
        );
        assert_eq!(
            clone_assets(Some(g), None, b, m),
            Some(given.join("assets"))
        );
        assert_eq!(clone_assets(Some(""), Some(m), b, m), None);
        assert_eq!(
            clone_assets(None, Some(m), b, ""),
            Some(main.join("assets"))
        );
        for dir in [built, main, given] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn a_clone_marked_untrusted_is_read_from_never() {
        let dir = clone_with_assets("untrusted");
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(args)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-q"]);
        git(&["config", "extensions.worktreeConfig", "true"]);
        git(&["config", "--worktree", crate::trust::MARK, "true"]);
        assert_eq!(clone_assets(None, None, dir.to_str().unwrap(), ""), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn this_checkouts_skills_are_what_build_rs_compiled_in() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/skills");
        let walked = skills_in(&root);
        let compiled: Vec<(String, Vec<u8>)> = crate::setup::skills::FILES
            .iter()
            .map(|(p, b)| (p.to_string(), b.to_vec()))
            .collect();
        assert_eq!(walked.len(), compiled.len());
        assert!(walked == compiled, "the walk and build.rs disagree");
    }

    #[test]
    fn a_skill_renamed_moves_the_fingerprint() {
        let dir = clone_with_assets("renamed").join("assets");
        std::fs::create_dir_all(dir.join("skills/a")).unwrap();
        std::fs::write(dir.join("skills/a/SKILL.md"), "a").unwrap();
        let before = fingerprint(&dir);
        assert_eq!(fingerprint(&dir), before);
        std::fs::rename(dir.join("skills/a"), dir.join("skills/b")).unwrap();
        assert_ne!(fingerprint(&dir), before);
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    #[test]
    fn a_changed_file_is_read_again() {
        let dir = clone_with_assets("cached");
        let file = dir.join("assets/x.md");
        std::fs::write(&file, "one").unwrap();
        assert_eq!(read_cached(&file).as_deref(), Some("one"));
        std::fs::write(&file, "three").unwrap();
        assert_eq!(read_cached(&file).as_deref(), Some("three"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
