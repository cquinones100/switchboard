//! The M-p palette (tmux/palette.sh): every command with a description, by
//! its description, in a list to type in; picking one runs it, exactly as
//! its key does. The list is read from the bindings, not kept anywhere: each
//! action's note (`Action::note`, its words and `#tags`) is what the palette
//! shows, its keys in a column of their own, Shift spelled out (`M-S-L`), and
//! actions no key is bound to (`Action::palette_only`) are listed with none.
//!
//! The commands picked last come first, newest first (`recent`, the last
//! six); the rest follow by description. As you type, a command picked
//! before after the same words comes first, then one picked after longer
//! words starting with them ("sha" for "shape"), each by how often, then by
//! how well each word typed fits a word of the row: 3 for the whole word, 2
//! for its start, 1 inside it (`rank`, palette.sh's `--rank`). What was typed
//! before each pick is kept (`queries`, the last 300) with the palette_learn
//! setting on. Both files are under `~/.local/state/switchboard/palette/`, in
//! a directory only you can read, since what you type names your work.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::mission::ViewId;
use crate::mux::keys::{Action, Bindings};
use crate::usage::serve::PluginView;
use crate::{paths, util};

use super::picker::PickItem;

/// How many recent picks lead the list.
pub const RECENT_MAX: usize = 6;
/// How many of what was typed before a pick are kept.
pub const QUERIES_MAX: usize = 300;

/// One command in the palette.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub action: Action,
    /// What it does, in words.
    pub words: String,
    /// Its tags, `#panes #split`.
    pub tags: String,
    /// The keys bound to it, as tmux's notation writes them, root table
    /// first; the prefix's written after the prefix key.
    pub keys: Vec<String>,
}

impl Entry {
    pub fn item(&self) -> PickItem {
        PickItem {
            text: self.words.clone(),
            tags: self.tags.clone(),
            key: self
                .keys
                .iter()
                .map(|k| shown_key(k))
                .collect::<Vec<_>>()
                .join(" "),
            value: self.action.name(),
        }
    }

    /// What UX 2.0's palette shows under its list while this command is
    /// chosen (board 15): what it does, more on it (`Action::detail`), and
    /// its key, or where to bind one.
    pub fn preview(&self) -> String {
        let keys = self.item().key;
        let key = if keys.is_empty() {
            "No key: bind one in keys.json".to_string()
        } else {
            format!("Key: {keys}")
        };
        format!(
            "{}
{}
{key}",
            self.words,
            self.action.detail()
        )
    }
}

/// A key as the palette shows it: tmux spells Shift with a letter as the
/// capital (Option+Shift+J is `M-J`), which reads as Option+J, so the
/// capital is spelled `S-J` (palette.sh's `show_key`).
pub fn shown_key(keys: &str) -> String {
    keys.split(' ')
        .map(|token| {
            let last = token.rsplit('-').next().unwrap_or(token);
            if last.len() == 1 && last.as_bytes()[0].is_ascii_uppercase() {
                format!("{}S-{last}", &token[..token.len() - 1])
            } else {
                token.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Every command the palette offers, by description: the bound actions
/// with a note and the actions with no key of their own, leaving out the
/// palette itself (picking it would only open it again) and a command
/// tagged `#github` or `#linear` while that service is off; and a
/// "Mission control: <view>" for each sidebar plugin's view in `plugins`
/// (the server's), by the title its plugin gives it now.
pub fn entries(
    bindings: &Bindings,
    github: bool,
    linear: bool,
    plugins: &[PluginView],
) -> Vec<Entry> {
    let mut by_action: Vec<Entry> = Vec::new();
    let mut add = |action: &Action, key: Option<String>, root: bool| {
        if *action == Action::Palette {
            return;
        }
        let Some((words, tags)) = action.palette_note() else {
            return;
        };
        let at = match by_action.iter().position(|e| e.action == *action) {
            Some(at) => at,
            None => {
                by_action.push(Entry {
                    action: action.clone(),
                    words,
                    tags,
                    keys: Vec::new(),
                });
                by_action.len() - 1
            }
        };
        if let Some(key) = key {
            // A key pressed alone is shown over one after the prefix.
            let entry = &mut by_action[at];
            if root {
                entry.keys.retain(|k| !k.contains(' '));
                entry.keys.push(key);
            } else if entry.keys.is_empty() || entry.keys.iter().all(|k| k.contains(' ')) {
                entry.keys.push(key);
            }
        }
    };
    let mut root: Vec<_> = bindings
        .root
        .iter()
        .map(|(k, a)| (k.notation(), a))
        .collect();
    root.sort_by(|a, b| a.0.cmp(&b.0));
    for (key, action) in &root {
        add(action, Some(key.clone()), true);
    }
    let prefix_key = bindings.prefix_key.notation();
    let mut prefix: Vec<_> = bindings
        .prefix
        .iter()
        .map(|(k, a)| (format!("{prefix_key} {}", k.notation()), a))
        .collect();
    prefix.sort_by(|a, b| a.0.cmp(&b.0));
    for (key, action) in &prefix {
        add(action, Some(key.clone()), false);
    }
    for action in Action::palette_only() {
        add(&action, None, false);
    }
    by_action.extend(plugins.iter().map(|p| {
        let action = Action::Extra(crate::extras::Command::Mission(ViewId::Plugin(
            p.name.clone(),
        )));
        let (_, tags) = action.palette_note().unwrap_or_default();
        Entry {
            words: format!("Mission control: {}, a sidebar plugin's view", p.title),
            action,
            tags,
            keys: Vec::new(),
        }
    }));
    let off = |tags: &str, tag: &str| tags.split(' ').any(|t| t == tag);
    by_action
        .retain(|e| (github || !off(&e.tags, "#github")) && (linear || !off(&e.tags, "#linear")));
    by_action.sort_by(|a, b| {
        (&a.words, &a.tags)
            .cmp(&(&b.words, &b.tags))
            .then_with(|| a.action.name().cmp(&b.action.name()))
    });
    by_action
}

/// While autopilot works `note` (its path), its command's words say what
/// picking it does then: change or stop the mode, after the command's name,
/// so typing the name still finds it. Its action, tags and keys stay as they
/// are, since the recent picks and its key go by the action.
pub fn autopilot_on(entries: &mut [Entry], note: &str) {
    let work_note = Action::Autopilot(crate::autopilot::Command::WorkNote);
    for entry in entries.iter_mut().filter(|e| e.action == work_note) {
        entry.words = format!(
            "Autopilot: work a note: change or stop working {}: its tabs at once, the most open, the spend limit, the branch it lands on, pause or stop",
            crate::autopilot::mode::note_title(note)
        );
    }
}

/// What a command listed only while Notes is in front does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotesPick {
    /// Autopilot working the note, by its path (`Server::work_note`).
    WorkNote(String),
    /// One of the app's own palette commands, by its words, run in the app
    /// (`run` on its socket).
    Run(String),
}

/// A command listed only while Notes is in front, ahead of the rest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotesEntry {
    pub item: PickItem,
    pub preview: String,
    pub pick: NotesPick,
}

/// The commands listed while Notes is in front, from what the app answered
/// (`front`): "Autopilot: work “<note>”" with a note shown and the
/// autopilot setting on, then the app's own commands, the checklist item's
/// first, each "Notes: <words>" with its key in Notes, with the
/// notes_palette setting on.
pub fn notes_entries(
    front: &crate::notes::socket::InFront,
    autopilot: bool,
    notes_palette: bool,
) -> Vec<NotesEntry> {
    let mut list = Vec::new();
    let note = front
        .note
        .as_ref()
        .filter(|n| autopilot && crate::notes::serve::in_notes(n));
    if let Some(note) = note {
        let words = format!(
            "Autopilot: work “{}”",
            crate::autopilot::mode::note_title(note)
        );
        list.push(NotesEntry {
            preview: format!(
                "{words}
Keeps tabs working through this note's checklist, the next opened as each lands. Already working it, its menu; working another note, asks whether to switch, keeping its counts, spend limit and landing; off, asks the repository first.
No key: Notes in front lists it"
            ),
            item: PickItem {
                text: words,
                tags: "#notes #autopilot".into(),
                key: String::new(),
                value: "notes:autopilot".into(),
            },
            pick: NotesPick::WorkNote(note.clone()),
        });
    }
    if notes_palette {
        list.extend(front.commands.iter().map(|c| {
            let words = format!("Notes: {}", c.name);
            let key = if c.key.is_empty() {
                "No key in Notes".to_string()
            } else {
                format!("Key in Notes: {}", c.key)
            };
            NotesEntry {
                preview: format!(
                    "{words}\nNotes' own command, run in Notes as its key would.\n{key}"
                ),
                item: PickItem {
                    text: words,
                    tags: format!("#notes #{}", c.tag),
                    key: c.key.clone(),
                    value: format!("notes:{}", c.name),
                },
                pick: NotesPick::Run(c.name.clone()),
            }
        }));
    }
    list
}

/// The entries with the ones picked last first, newest first.
pub fn recent_first(mut entries: Vec<Entry>, recent: &[String]) -> Vec<Entry> {
    let rank = |e: &Entry| {
        recent
            .iter()
            .position(|r| *r == e.action.name())
            .unwrap_or(usize::MAX)
    };
    // A stable sort keeps the rest by description.
    entries.sort_by_key(rank);
    entries
}

/// What was typed, as it is kept and compared: lowercase, the spaces
/// between words one each, none at the ends.
pub fn normalize(query: &str) -> String {
    query
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Orders the rows that matched `query`, best first (palette.sh's
/// `--rank`): `rows` is each row's value (its action's name) and the text
/// it is matched on, in the list's order; `picks` is what was typed before
/// each pick and what was picked. A row picked after this same query comes
/// first, then one picked after a longer query this one starts, each tier
/// by how often; then by how each word typed fits the row's words (3 a
/// whole word, 2 its start, 1 inside it, summed); ties keep the list's order.
pub fn rank(query: &str, rows: &[(String, String)], picks: &[(String, String)]) -> Vec<usize> {
    let typed = normalize(query);
    let terms: Vec<&str> = typed.split(' ').filter(|t| !t.is_empty()).collect();
    let mut same: HashMap<&str, usize> = HashMap::new();
    let mut longer: HashMap<&str, usize> = HashMap::new();
    if !typed.is_empty() {
        for (past, value) in picks {
            if *past == typed {
                *same.entry(value.as_str()).or_default() += 1;
            } else if past.starts_with(&typed) {
                *longer.entry(value.as_str()).or_default() += 1;
            }
        }
    }
    let mut keyed: Vec<(usize, usize, usize, usize)> = rows
        .iter()
        .enumerate()
        .map(|(i, (value, text))| {
            let lowered = text.to_lowercase();
            let words: Vec<&str> = lowered
                .split(|c: char| !c.is_ascii_alphanumeric())
                .filter(|w| !w.is_empty())
                .collect();
            let score = terms
                .iter()
                .map(|term| {
                    words
                        .iter()
                        .map(|w| {
                            if w == term {
                                3
                            } else if w.starts_with(term) {
                                2
                            } else if w.contains(term) {
                                1
                            } else {
                                0
                            }
                        })
                        .max()
                        .unwrap_or(0)
                })
                .sum();
            (
                same.get(value.as_str()).copied().unwrap_or(0),
                longer.get(value.as_str()).copied().unwrap_or(0),
                score,
                i,
            )
        })
        .collect();
    keyed.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(b.1.cmp(&a.1))
            .then(b.2.cmp(&a.2))
            .then(a.3.cmp(&b.3))
    });
    keyed.into_iter().map(|(_, _, _, i)| i).collect()
}

fn dir() -> PathBuf {
    paths::state_dir().join("palette")
}

pub fn recent_file() -> PathBuf {
    dir().join("recent")
}

pub fn queries_file() -> PathBuf {
    dir().join("queries")
}

/// The actions picked last, newest first.
pub fn read_recent() -> Vec<String> {
    util::read_text(recent_file())
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// What was typed before each pick, and what was picked, oldest first.
pub fn read_queries() -> Vec<(String, String)> {
    util::read_text(queries_file())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(q, v)| (q.to_string(), v.to_string()))
        .collect()
}

/// Keeps a pick: first among the recent ones, and, when `learn` and
/// something was typed, after what was typed.
pub fn remember(value: &str, query: &str, learn: bool) {
    let dir = dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let mut recent = vec![value.to_string()];
    recent.extend(read_recent().into_iter().filter(|r| r != value));
    recent.truncate(RECENT_MAX);
    let _ = util::write_atomic(recent_file(), format!("{}\n", recent.join("\n")));
    learn_query(value, query, learn);
}

/// Keeps what was typed before a pick, when `learn` and something was
/// typed, so the same words rank it first again; a command listed only
/// while Notes is in front is kept so alone, its place among the recent
/// picks left to the workspace's commands, which lead the list.
pub fn learn_query(value: &str, query: &str, learn: bool) {
    let dir = dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let typed = normalize(query);
    if learn && !typed.is_empty() {
        let mut queries = read_queries();
        queries.push((typed, value.to_string()));
        let skip = queries.len().saturating_sub(QUERIES_MAX);
        let text: String = queries[skip..]
            .iter()
            .map(|(q, v)| format!("{q}\t{v}\n"))
            .collect();
        let _ = util::write_atomic(queries_file(), text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::Direction;

    fn rows(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(v, t)| (v.to_string(), t.to_string()))
            .collect()
    }

    #[test]
    fn shift_is_spelled_out() {
        assert_eq!(shown_key("M-L"), "M-S-L");
        assert_eq!(shown_key("M-b d"), "M-b d");
        assert_eq!(shown_key("M-b W"), "M-b S-W");
        assert_eq!(shown_key("M-["), "M-[");
        assert_eq!(shown_key("M-S-Left"), "M-S-Left");
    }

    #[test]
    fn whole_words_rank_over_starts_and_insides() {
        let list = rows(&[
            ("a", "Previous tab #tabs"),
            ("b", "Open the PR #github #pr"),
            ("c", "Split pane to the right #panes"),
        ]);
        assert_eq!(rank("pr", &list, &[]), vec![1, 0, 2]);
        assert_eq!(
            rank("", &list, &[]),
            vec![0, 1, 2],
            "nothing typed keeps the order"
        );
    }

    #[test]
    fn what_was_picked_after_the_same_words_comes_first() {
        let list = rows(&[
            ("split-left", "Split pane to the left"),
            ("split-right", "Split pane to the right"),
        ]);
        let picks = vec![
            ("split".to_string(), "split-right".to_string()),
            ("spl".to_string(), "split-left".to_string()),
            ("spl".to_string(), "split-left".to_string()),
        ];
        assert_eq!(rank("Split", &list, &picks), vec![1, 0]);
        assert_eq!(
            rank("sp", &list, &picks),
            vec![0, 1],
            "two picks after longer words beat one"
        );
    }

    #[test]
    fn the_palette_lists_each_command_once_with_its_keys() {
        let all = entries(&Bindings::default(), true, true, &[]);
        assert!(all.iter().all(|e| e.action != Action::Palette));
        let split = all
            .iter()
            .find(|e| e.action == Action::Split(Direction::Right))
            .unwrap();
        assert_eq!(split.words, "Split pane to the right");
        assert_eq!(split.tags, "#panes #split");
        assert_eq!(split.keys, vec!["M-L"], "the root key over the prefix's");
        assert_eq!(split.item().key, "M-S-L");
        let detach = all.iter().find(|e| e.action == Action::Detach).unwrap();
        assert_eq!(detach.keys, vec!["M-b d"]);
        let tips = all.iter().find(|e| e.action == Action::ResetTips).unwrap();
        assert!(tips.keys.is_empty(), "a command with no key of its own");
        assert_eq!(
            all.iter()
                .filter(|e| matches!(e.action, Action::SelectTab(_)))
                .count(),
            1,
            "one entry for M-1 to M-9"
        );
        let words: Vec<_> = all.iter().map(|e| e.words.clone()).collect();
        let mut sorted = words.clone();
        sorted.sort();
        assert_eq!(words, sorted, "by description");
    }

    #[test]
    fn every_command_previews_more_than_its_words_and_its_key() {
        let all = entries(&Bindings::default(), true, true, &[]);
        for e in &all {
            let detail = e.action.detail();
            assert!(!detail.is_empty(), "{} has no detail", e.action.name());
            assert_ne!(detail, e.words, "{} repeats its words", e.action.name());
        }
        let split = all
            .iter()
            .find(|e| e.action == Action::Split(Direction::Right))
            .unwrap();
        let preview = split.preview();
        let lines: Vec<&str> = preview.lines().collect();
        assert_eq!(lines[0], "Split pane to the right");
        assert_eq!(lines[2], "Key: M-S-L");
        let tips = all.iter().find(|e| e.action == Action::ResetTips).unwrap();
        assert!(tips.preview().ends_with("No key: bind one in keys.json"));
    }

    #[test]
    fn every_built_in_view_and_each_plugins_view_is_one_pick_away() {
        let plugin = PluginView {
            name: "deploys".into(),
            title: "Deploys".into(),
            sidebar: 0,
            loaded: true,
            data: Default::default(),
        };
        let all = entries(&Bindings::default(), true, true, &[plugin]);
        let mission = |id: ViewId| Action::Extra(crate::extras::Command::Mission(id));
        for id in ViewId::BUILT_IN.into_iter().filter(|v| v.shows()) {
            let entry = all.iter().find(|e| e.action == mission(id.clone()));
            assert!(entry.is_some(), "no entry for {}", id.title());
            assert!(entry.unwrap().tags.contains("#mission"));
        }
        let deploys = all
            .iter()
            .find(|e| e.action == mission(ViewId::Plugin("deploys".into())))
            .unwrap();
        assert_eq!(
            deploys.words,
            "Mission control: Deploys, a sidebar plugin's view"
        );
        let words: Vec<_> = all.iter().map(|e| e.words.clone()).collect();
        let mut sorted = words.clone();
        sorted.sort();
        assert_eq!(words, sorted, "the plugin's in its place by description");
    }

    #[test]
    fn recent_picks_lead() {
        let all = entries(&Bindings::default(), true, true, &[]);
        let ordered = recent_first(all, &["zoom".into(), "detach".into()]);
        assert_eq!(ordered[0].action, Action::Zoom);
        assert_eq!(ordered[1].action, Action::Detach);
    }

    #[test]
    fn autopilot_on_names_the_note_and_keeps_the_rest() {
        let work_note = Action::Autopilot(crate::autopilot::Command::WorkNote);
        let all = entries(&Bindings::default(), true, true, &[]);
        let before = all.iter().find(|e| e.action == work_note).unwrap();
        assert_eq!(before.words, crate::autopilot::Command::WorkNote.about());

        let mut on = all.clone();
        autopilot_on(&mut on, "Work/Ship it.md");
        let entry = on.iter().find(|e| e.action == work_note).unwrap();
        assert!(
            entry
                .words
                .starts_with("Autopilot: work a note: change or stop working Ship it:"),
            "{}",
            entry.words
        );
        assert_eq!(
            (&entry.action, &entry.tags, &entry.keys, &entry.item().value),
            (
                &before.action,
                &before.tags,
                &before.keys,
                &before.item().value
            )
        );
        for (a, b) in all.iter().zip(&on) {
            if a.action != work_note {
                assert_eq!(a, b);
            }
        }
    }

    #[test]
    fn notes_in_front_lists_autopilot_on_its_note_then_its_own_commands() {
        use crate::notes::socket::{InFront, Listed};
        let front = InFront {
            note: Some("Work/Plan.md".into()),
            commands: vec![Listed {
                name: "Add an item".into(),
                key: "ga".into(),
                tag: "note".into(),
            }],
        };
        let all = notes_entries(&front, true, true);
        let words: Vec<&str> = all.iter().map(|n| n.item.text.as_str()).collect();
        assert_eq!(words, ["Autopilot: work “Plan”", "Notes: Add an item"]);
        assert_eq!(all[0].pick, NotesPick::WorkNote("Work/Plan.md".into()));
        assert_eq!(all[1].pick, NotesPick::Run("Add an item".into()));
        assert_eq!(
            (all[1].item.key.as_str(), all[1].item.tags.as_str()),
            ("ga", "#notes #note")
        );
        assert_eq!(notes_entries(&front, false, true).len(), 1, "autopilot off");
        assert_eq!(
            notes_entries(&front, true, false).len(),
            1,
            "notes_palette off"
        );
        let listless = InFront {
            note: None,
            ..front
        };
        assert_eq!(
            notes_entries(&listless, true, false),
            Vec::new(),
            "no note shown"
        );
    }
}
