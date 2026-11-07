//! The mode's state, what you chose, kept in `autopilot.json` in the state
//! directory as lib/sidebar/autopilot.js kept it: `{ on, note, repo, slots,
//! cap, paused, readyOnly, skipped, spendLimit, said }`. `note` is the
//! note's path within the notes directory, `repo` the main clone its items
//! are worked in, `skipped` the first lines of the items skipped, and `said`
//! what the notifications show for it.
//!
//! The notes app writes the file too, when a note it works is renamed or
//! moved (src/notes/store.rs's `relink_mode`), so it is read again on every
//! check rather than kept, and written back with whatever keys it holds
//! that this doesn't know. A file that isn't there or can't be read is the
//! mode off with its defaults; a `skipped` that isn't a list (Lua's JSON
//! once wrote an empty one as an object) is none.

use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::util;

/// The file, in the state directory.
pub const FILE: &str = "autopilot.json";

/// What you chose.
#[derive(Clone, Debug, PartialEq)]
pub struct Mode {
    pub on: bool,
    /// The note's path within the notes directory, ".md" and all.
    pub note: String,
    /// The main clone of the note's repository; empty before it is set up.
    pub repo: PathBuf,
    /// How many of its tabs work at once.
    pub slots: u32,
    /// The most of its tabs open at once, those waiting on you included.
    pub cap: u32,
    pub paused: bool,
    /// Only items whose plan is settled.
    pub ready_only: bool,
    /// The items skipped, by their first line.
    pub skipped: Vec<String>,
    /// Today's Claude spend, in dollars, past which it opens no more tabs;
    /// 0 for none.
    pub spend_limit: f64,
    /// What it has to say, in the notifications; empty for nothing.
    pub said: String,
}

impl Default for Mode {
    fn default() -> Mode {
        Mode {
            on: false,
            note: String::new(),
            repo: PathBuf::new(),
            slots: 3,
            cap: 5,
            paused: false,
            ready_only: false,
            skipped: Vec::new(),
            spend_limit: 0.0,
            said: String::new(),
        }
    }
}

impl Mode {
    pub fn file(state: &Path) -> PathBuf {
        state.join(FILE)
    }

    /// The mode from the file's object, each key its default when it is
    /// missing or of another kind.
    pub fn from_map(map: &Map<String, Value>) -> Mode {
        let d = Mode::default();
        let flag = |k: &str, or: bool| map.get(k).and_then(Value::as_bool).unwrap_or(or);
        let text = |k: &str| map.get(k).and_then(Value::as_str).map(str::to_string);
        let count = |k: &str, or: u32| {
            map.get(k)
                .and_then(Value::as_f64)
                .filter(|n| n.is_finite() && *n >= 0.0)
                .map_or(or, |n| n as u32)
        };
        Mode {
            on: flag("on", d.on),
            note: text("note").unwrap_or_default(),
            repo: text("repo").map(PathBuf::from).unwrap_or_default(),
            slots: count("slots", d.slots),
            cap: count("cap", d.cap),
            paused: flag("paused", d.paused),
            ready_only: flag("readyOnly", d.ready_only),
            skipped: match map.get("skipped") {
                Some(Value::Array(list)) => list
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect(),
                _ => Vec::new(),
            },
            spend_limit: map
                .get("spendLimit")
                .and_then(Value::as_f64)
                .filter(|n| n.is_finite() && *n >= 0.0)
                .unwrap_or(0.0),
            said: text("said").unwrap_or_default(),
        }
    }

    /// The mode as the file has it now.
    pub fn read(state: &Path) -> Mode {
        Mode::from_map(&util::read_json_object(Mode::file(state)))
    }

    /// Its keys written into the file's object, as Node wrote them.
    fn into_map(self, map: &mut Map<String, Value>) {
        map.insert("on".into(), self.on.into());
        map.insert("note".into(), self.note.into());
        map.insert(
            "repo".into(),
            self.repo.to_string_lossy().into_owned().into(),
        );
        map.insert("slots".into(), self.slots.into());
        map.insert("cap".into(), self.cap.into());
        map.insert("paused".into(), self.paused.into());
        map.insert("readyOnly".into(), self.ready_only.into());
        map.insert(
            "skipped".into(),
            Value::Array(self.skipped.into_iter().map(Value::from).collect()),
        );
        map.insert("spendLimit".into(), util::js_number(self.spend_limit));
        map.insert("said".into(), self.said.into());
    }

    /// Reads the file, changes the mode with `change`, and writes it back
    /// whole, with any key it holds that the mode doesn't know; gives the
    /// mode as written.
    pub fn update(state: &Path, change: impl FnOnce(&mut Mode)) -> io::Result<Mode> {
        let file = Mode::file(state);
        let mut map = util::read_json_object(&file);
        let mut mode = Mode::from_map(&map);
        change(&mut mode);
        mode.clone().into_map(&mut map);
        util::write_atomic(&file, util::json_pretty(&Value::Object(map)))?;
        Ok(mode)
    }

    /// Whether it is set up to work a note.
    pub fn set_up(&self) -> bool {
        !self.repo.as_os_str().is_empty()
    }
}

/// A note's title, by its path: its file's name without ".md".
pub fn note_title(note: &str) -> String {
    let name = Path::new(note)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    name.strip_suffix(".md").unwrap_or(&name).to_string()
}

/// The notes to pick from, by their paths within the notes directory with
/// ".md": the one the mode last worked first, then the most recently
/// changed, leaving out deleted ones and every other dot-folder. Reads the
/// whole directory, so it is called off the server's task.
pub fn note_choices(notes: &Path, last: &str) -> Vec<String> {
    let last = last.strip_suffix(".md").unwrap_or(last);
    let mut names = crate::cmd::worktree_from_note::notes_newest_first(notes);
    if let Some(at) = names.iter().position(|n| n == last) {
        let found = names.remove(at);
        names.insert(0, found);
    }
    names.into_iter().map(|n| format!("{n}.md")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sb-autopilot-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_file_missing_or_odd_is_the_mode_off_with_its_defaults() {
        let dir = temp("defaults");
        assert_eq!(Mode::read(&dir), Mode::default());
        fs::write(
            Mode::file(&dir),
            r#"{"on":true,"note":"Work.md","slots":"x","skipped":{},"spendLimit":2.5}"#,
        )
        .unwrap();
        let mode = Mode::read(&dir);
        assert!(mode.on);
        assert_eq!(mode.slots, 3, "a count that isn't one is the default");
        assert_eq!(mode.skipped, Vec::<String>::new(), "Lua's empty object");
        assert_eq!(mode.spend_limit, 2.5);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_update_keeps_the_keys_it_doesnt_know_and_writes_numbers_as_node_did() {
        let dir = temp("update");
        fs::write(Mode::file(&dir), r#"{"note":"Work.md","later":1}"#).unwrap();
        let mode = Mode::update(&dir, |m| {
            m.on = true;
            m.spend_limit = 20.0;
            m.skipped.push("One".into());
        })
        .unwrap();
        assert!(mode.on);
        let written = util::read_json(Mode::file(&dir)).unwrap();
        assert_eq!(written["later"], 1);
        assert_eq!(written["spendLimit"], 20);
        assert_eq!(written["readyOnly"], false);
        assert_eq!(written["skipped"][0], "One");
        assert_eq!(written["note"], "Work.md");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_note_last_worked_comes_first_then_the_newest() {
        let dir = temp("choices");
        fs::create_dir_all(dir.join("Work")).unwrap();
        fs::create_dir_all(dir.join(".trash")).unwrap();
        for (name, ago) in [("Old.md", 300), ("Work/New.md", 10), (".trash/Gone.md", 0)] {
            let file = dir.join(name);
            fs::write(&file, "# x\n").unwrap();
            let at = std::time::SystemTime::now() - std::time::Duration::from_secs(ago);
            fs::File::options()
                .write(true)
                .open(&file)
                .unwrap()
                .set_modified(at)
                .unwrap();
        }
        assert_eq!(note_choices(&dir, ""), vec!["Work/New.md", "Old.md"]);
        assert_eq!(note_choices(&dir, "Old.md"), vec!["Old.md", "Work/New.md"]);
        assert_eq!(note_title("Work/Plan's Notes.md"), "Plan's Notes");
        let _ = fs::remove_dir_all(&dir);
    }
}
