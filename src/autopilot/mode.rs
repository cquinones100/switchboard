//! The mode's state, what you chose, kept in `autopilot.json` in the state
//! directory as lib/sidebar/autopilot.js kept it: `{ on, note, noteId, repo,
//! slots, cap, paused, readyOnly, skipped, spendLimit, landWithoutAsking,
//! said, closed }`.
//! `note` is the note's path within the notes directory and `noteId` its
//! stable ID (src/notes/id.rs), which the mode follows the note by when it
//! has been renamed or moved, outside Notes too, writing its path back;
//! `repo` the main clone its items are worked in, `skipped` the items
//! skipped, `[{ note, text }]`, each by its note's ID and first line, so a
//! skipped item hides only in its own note (a first line alone, from before
//! notes had IDs, was the mode's note's); `landWithoutAsking` whether its
//! tabs land on the default branch without asking to push (its menu's
//! "Land without asking"); `said` what the notifications show
//! for it, and `closed` the landed tabs it closed without your having seen
//! them, `[{ id, item, hash, at }]`, each `id` the tab's and its `@autopilot`
//! time, for the notifications.
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
    /// The note's stable ID; empty until it is given one.
    pub note_id: String,
    /// The main clone of the note's repository; empty before it is set up.
    pub repo: PathBuf,
    /// How many of its tabs work at once.
    pub slots: u32,
    /// The most of its tabs open at once, those waiting on you included.
    pub cap: u32,
    pub paused: bool,
    /// Only items whose plan is settled.
    pub ready_only: bool,
    /// The items skipped, each by its note's ID and its first line.
    pub skipped: Vec<Skipped>,
    /// Today's Claude spend, in dollars, past which it opens no more tabs;
    /// 0 for none.
    pub spend_limit: f64,
    /// Its tabs land on the default branch without asking to push.
    pub land_without_asking: bool,
    /// The branch its tabs are cut from and land on; empty for the
    /// repository's default branch.
    pub target: String,
    /// What it has to say, in the notifications; empty for nothing.
    pub said: String,
    /// The landed tabs it closed without your seeing them, the last
    /// `CLOSED_KEPT`, oldest first.
    pub closed: Vec<Closed>,
}

/// An item skipped: its note's ID and its first line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    pub note: String,
    pub text: String,
}

/// A landed tab closed without your having seen it.
#[derive(Clone, Debug, PartialEq)]
pub struct Closed {
    /// The tab and when it opened (`@autopilot`), since a tab's id comes
    /// round again after a restart.
    pub id: String,
    /// The item's first line, else the tab's name.
    pub item: String,
    /// The main clone's commit as its Claude's pane found it there, short.
    pub hash: String,
    /// When it closed (ms).
    pub at: i64,
}

impl Closed {
    /// "Closed <item>: landed <short hash>".
    pub fn title(&self) -> String {
        if self.hash.is_empty() {
            format!("Closed {}", self.item)
        } else {
            format!("Closed {}: landed {}", self.item, self.hash)
        }
    }
}

/// The tabs closed unseen kept for the notifications.
pub const CLOSED_KEPT: usize = 20;

impl Default for Mode {
    fn default() -> Mode {
        Mode {
            on: false,
            note: String::new(),
            note_id: String::new(),
            repo: PathBuf::new(),
            slots: 3,
            cap: 5,
            paused: false,
            ready_only: false,
            skipped: Vec::new(),
            spend_limit: 0.0,
            land_without_asking: false,
            target: String::new(),
            said: String::new(),
            closed: Vec::new(),
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
        let note_id = text("noteId").unwrap_or_default();
        Mode {
            on: flag("on", d.on),
            note: text("note").unwrap_or_default(),
            repo: text("repo").map(PathBuf::from).unwrap_or_default(),
            slots: count("slots", d.slots),
            cap: count("cap", d.cap),
            paused: flag("paused", d.paused),
            ready_only: flag("readyOnly", d.ready_only),
            // A first line alone, from before notes had IDs, was the mode's
            // note's.
            skipped: match map.get("skipped") {
                Some(Value::Array(list)) => list
                    .iter()
                    .filter_map(|x| match x {
                        Value::String(text) => Some(Skipped {
                            note: note_id.clone(),
                            text: text.clone(),
                        }),
                        Value::Object(o) => Some(Skipped {
                            note: o.get("note").and_then(Value::as_str)?.to_string(),
                            text: o.get("text").and_then(Value::as_str)?.to_string(),
                        }),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            },
            spend_limit: map
                .get("spendLimit")
                .and_then(Value::as_f64)
                .filter(|n| n.is_finite() && *n >= 0.0)
                .unwrap_or(0.0),
            land_without_asking: flag("landWithoutAsking", d.land_without_asking),
            target: text("target").unwrap_or_default(),
            said: text("said").unwrap_or_default(),
            closed: match map.get("closed") {
                Some(Value::Array(list)) => list
                    .iter()
                    .filter_map(|c| {
                        let text = |k: &str| c.get(k).and_then(Value::as_str).unwrap_or("");
                        let id = text("id");
                        (!id.is_empty()).then(|| Closed {
                            id: id.to_string(),
                            item: text("item").to_string(),
                            hash: text("hash").to_string(),
                            at: c.get("at").and_then(Value::as_f64).unwrap_or(0.0) as i64,
                        })
                    })
                    .collect(),
                _ => Vec::new(),
            },
            note_id,
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
        map.insert("noteId".into(), self.note_id.into());
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
            Value::Array(
                self.skipped
                    .into_iter()
                    .map(|x| serde_json::json!({ "note": x.note, "text": x.text }))
                    .collect(),
            ),
        );
        map.insert("spendLimit".into(), util::js_number(self.spend_limit));
        map.insert("landWithoutAsking".into(), self.land_without_asking.into());
        map.insert("target".into(), self.target.into());
        map.insert("said".into(), self.said.into());
        map.insert(
            "closed".into(),
            Value::Array(
                self.closed
                    .into_iter()
                    .map(|c| {
                        serde_json::json!({
                            "id": c.id, "item": c.item, "hash": c.hash,
                            "at": util::js_number(c.at as f64),
                        })
                    })
                    .collect(),
            ),
        );
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

    /// The skipped items of the mode's note, by their first line.
    pub fn skipped_here(&self) -> impl Iterator<Item = &str> {
        self.skipped
            .iter()
            .filter(|x| x.note == self.note_id)
            .map(|x| x.text.as_str())
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
/// ".md", those holding an unticked item (`open`) apart from the rest, so
/// the menu lists the ones with work in them and keeps the others behind
/// "More notes…" (autopilot.js's `noteChoices`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NoteChoices {
    pub open: Vec<String>,
    pub rest: Vec<String>,
}

/// The value of "Another branch…" in the target's list, which no branch
/// can be named (git refuses a `:` in one).
pub const ANOTHER_BRANCH: &str = ":another";

/// "Land on:"'s list, each option's label and value: the branches picked
/// before (`recent`, from src/cmd/work_recent.rs, `""` for the default
/// one, or its name when typed), the most recent first, then the default
/// branch if it isn't among them, its value empty so the mode keeps
/// following it, then the other local branches by name, then "Another branch…" to type one. With nothing
/// picked yet the default branch leads, so Enter lands there.
pub fn target_choices(
    default: Option<&str>,
    branches: &[String],
    recent: &[String],
) -> Vec<(String, String)> {
    let default_row = || {
        (
            match default {
                Some(d) => format!("{d} (the default branch)"),
                None => "The default branch".to_string(),
            },
            String::new(),
        )
    };
    let others: Vec<&String> = branches
        .iter()
        .filter(|b| Some(b.as_str()) != default)
        .collect();
    let mut out: Vec<(String, String)> = Vec::new();
    for r in recent {
        // The default branch typed by its name through "Another branch…"
        // counts as a pick of the default one.
        let is_default = r.is_empty() || Some(r.as_str()) == default;
        if is_default {
            if !out.iter().any(|(_, v)| v.is_empty()) {
                out.push(default_row());
            }
        } else if others.contains(&r) && !out.iter().any(|(_, v)| v == r) {
            out.push((r.clone(), r.clone()));
        }
    }
    if !out.iter().any(|(_, v)| v.is_empty()) {
        out.push(default_row());
    }
    let mut rest: Vec<&String> = others
        .into_iter()
        .filter(|b| !out.iter().any(|(_, v)| v == *b))
        .collect();
    rest.sort();
    out.extend(rest.into_iter().map(|b| (b.clone(), b.clone())));
    out.push(("Another branch…".into(), ANOTHER_BRANCH.into()));
    out
}

/// The branch typed for "Another branch…", or `None` for one git couldn't
/// name (empty, a space, `..`, a `:` and the like).
pub fn typed_branch(text: &str) -> Option<String> {
    let t = text.trim();
    let bad = t.is_empty()
        || t.starts_with(['-', '/', '.'])
        || t.ends_with(['/', '.'])
        || t.ends_with(".lock")
        || t.contains("..")
        || t.contains("//")
        || t.contains("@{")
        || t.chars()
            .any(|c| c.is_whitespace() || c.is_control() || "~^:?*[\\".contains(c));
    (!bad).then(|| t.to_string())
}

/// The notes to pick from, each list the one the mode last worked first,
/// then the most recently changed, leaving out deleted ones and every other
/// dot-folder. Reads the whole directory and every note in it, so it is
/// called off the server's task.
pub fn note_choices(notes: &Path, last: &str) -> NoteChoices {
    let last = last.strip_suffix(".md").unwrap_or(last);
    let mut names = crate::cmd::worktree_from_note::notes_newest_first(notes);
    if let Some(at) = names.iter().position(|n| n == last) {
        let found = names.remove(at);
        names.insert(0, found);
    }
    let mut choices = NoteChoices::default();
    for name in names {
        let note = format!("{name}.md");
        let open = std::fs::read_to_string(notes.join(&note))
            .is_ok_and(|text| text.lines().any(|l| l.trim_start().starts_with("- [ ] ")));
        if open {
            choices.open.push(note);
        } else {
            choices.rest.push(note);
        }
    }
    choices
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
        assert_eq!(mode.skipped, Vec::<Skipped>::new(), "Lua's empty object");
        assert_eq!(mode.spend_limit, 2.5);
        assert_eq!(
            mode.target, "",
            "a state saved before targets lands on the default branch"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_update_keeps_the_keys_it_doesnt_know_and_writes_numbers_as_node_did() {
        let dir = temp("update");
        fs::write(Mode::file(&dir), r#"{"note":"Work.md","later":1}"#).unwrap();
        let mode = Mode::update(&dir, |m| {
            m.on = true;
            m.spend_limit = 20.0;
            m.note_id = "n-0123456789ab".into();
            m.target = "team/next".into();
            m.skipped.push(Skipped {
                note: m.note_id.clone(),
                text: "One".into(),
            });
        })
        .unwrap();
        assert!(mode.on);
        let written = util::read_json(Mode::file(&dir)).unwrap();
        assert_eq!(written["later"], 1);
        assert_eq!(written["spendLimit"], 20);
        assert_eq!(written["readyOnly"], false);
        assert_eq!(
            written["skipped"][0],
            serde_json::json!({ "note": "n-0123456789ab", "text": "One" })
        );
        assert_eq!(written["note"], "Work.md");
        assert_eq!(written["noteId"], "n-0123456789ab");
        assert_eq!(written["landWithoutAsking"], false);
        assert_eq!(written["target"], "team/next");
        assert_eq!(Mode::read(&dir).target, "team/next");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_skipped_item_from_before_notes_had_ids_is_the_modes_notes_and_hides_only_there() {
        let dir = temp("skipped");
        fs::write(
            Mode::file(&dir),
            r#"{"note":"Work.md","noteId":"n-0123456789ab","skipped":["Old",{"note":"n-ffffffffffff","text":"Elsewhere"},{"note":"n-0123456789ab","text":"Here"}]}"#,
        )
        .unwrap();
        let mode = Mode::read(&dir);
        assert_eq!(mode.skipped[0].note, "n-0123456789ab");
        assert_eq!(mode.skipped_here().collect::<Vec<_>>(), ["Old", "Here"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_landed_tabs_closed_unseen_are_kept_and_said_with_what_landed() {
        let dir = temp("closed");
        Mode::update(&dir, |m| {
            m.closed.push(Closed {
                id: "3@1700000000".into(),
                item: "Copy the reply".into(),
                hash: "abc1234".into(),
                at: 1,
            })
        })
        .unwrap();
        let mode = Mode::read(&dir);
        assert_eq!(mode.closed.len(), 1);
        assert_eq!(
            mode.closed[0].title(),
            "Closed Copy the reply: landed abc1234"
        );
        fs::write(Mode::file(&dir), r#"{"closed":{}}"#).unwrap();
        assert!(Mode::read(&dir).closed.is_empty(), "a list or none");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_note_last_worked_comes_first_then_the_newest() {
        let dir = temp("choices");
        fs::create_dir_all(dir.join("Work")).unwrap();
        fs::create_dir_all(dir.join(".trash")).unwrap();
        for (name, ago, text) in [
            ("Old.md", 300, "# x\n"),
            ("Work/New.md", 10, "# x\n"),
            (".trash/Gone.md", 0, "- [ ] gone\n"),
            ("Ticked.md", 200, "# x\n- [x] done\n"),
            ("Todo.md", 100, "# x\n- [x] done\n  - [ ] under it\n"),
            ("Plan.md", 400, "# x\n- [ ] first\n"),
        ] {
            let file = dir.join(name);
            fs::write(&file, text).unwrap();
            let at = std::time::SystemTime::now() - std::time::Duration::from_secs(ago);
            fs::File::options()
                .write(true)
                .open(&file)
                .unwrap()
                .set_modified(at)
                .unwrap();
        }
        let v = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        assert_eq!(
            note_choices(&dir, ""),
            NoteChoices {
                open: v(&["Todo.md", "Plan.md"]),
                rest: v(&["Work/New.md", "Ticked.md", "Old.md"]),
            },
            "those holding an unticked item apart, each newest first"
        );
        let last = note_choices(&dir, "Old.md");
        assert_eq!(last.rest, v(&["Old.md", "Work/New.md", "Ticked.md"]));
        assert_eq!(
            note_choices(&dir, "Plan.md").open,
            v(&["Plan.md", "Todo.md"])
        );
        assert_eq!(note_title("Work/Plan's Notes.md"), "Plan's Notes");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn land_on_lists_the_default_branch_first_then_the_others_then_another() {
        let branches = ["x".to_string(), "main".into(), "team/next".into()];
        assert_eq!(
            target_choices(Some("main"), &branches, &[]),
            [
                ("main (the default branch)".to_string(), String::new()),
                ("team/next".into(), "team/next".into()),
                ("x".into(), "x".into()),
                ("Another branch…".into(), ANOTHER_BRANCH.into()),
            ]
        );
        assert_eq!(target_choices(None, &[], &[])[0].0, "The default branch");
        assert_eq!(
            typed_branch(" release/1.2 ").as_deref(),
            Some("release/1.2")
        );
        for bad in ["", "a b", "a..b", "-x", "x:y", "x.lock", "x/"] {
            assert_eq!(typed_branch(bad), None, "{bad}");
        }
    }

    #[test]
    fn land_on_lists_the_branches_picked_before_first_the_most_recent_first() {
        let branches = [
            "z".to_string(),
            "main".into(),
            "team/next".into(),
            "a".into(),
        ];
        let values = |recent: &[&str]| -> Vec<String> {
            let recent: Vec<String> = recent.iter().map(|r| r.to_string()).collect();
            target_choices(Some("main"), &branches, &recent)
                .into_iter()
                .map(|(_, v)| v)
                .collect()
        };
        // The default branch by its own time among them, a branch gone left
        // out, the rest by name.
        assert_eq!(
            values(&["z", "", "gone", "team/next"]),
            ["z", "", "team/next", "a", ANOTHER_BRANCH]
        );
        // The default branch typed by its name counts as picked, once.
        assert_eq!(
            values(&["main", "z", ""]),
            ["", "z", "a", "team/next", ANOTHER_BRANCH]
        );
        // Never picked, the default branch leads the rest.
        assert_eq!(
            values(&["team/next"]),
            ["team/next", "", "a", "z", ANOTHER_BRANCH]
        );
    }
}
