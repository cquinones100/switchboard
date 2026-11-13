//! Mission control's Keys view (M-/; lib/sidebar/keymap.js and
//! lib/sidebar/app-keys.json): every key Switchboard binds, in one list
//! under headings, its groups. First the workspace's, as the M-p palette
//! lists them (`ui::palette::entries`, M-p itself included), each under the
//! first of its tags, the biggest group first; then the sidebar's and
//! mission control's own (the lists their `?` shows), M-d's diff's
//! (`diff::app::KEYS`, the cover, the box `ga` opens and its answer, those
//! only with diff_ask) and Notes' (`notes::app::key_groups`: a note's, with
//! its leader keys and, while easy_keys is on, easy mode's Cmd keys, then
//! the list's), each from the table its own code keeps; last the commands
//! no key is bound to, reached only through M-p. A key column as wide as
//! the widest key cuts none (`render`).
//!
//! Each row carries its group, its tags whole, whether `keys.json` bound it
//! (`Bindings::yours`, "yours") and how often it was pressed this week, from
//! the habits' counts (src/usage/habits.rs) the server read off its task,
//! and the view's head how often each Alt key was, by its letter, for the
//! heatmap UX 2.0's board draws (`keys_board`).
//!
//! Its `/` filter matches a key, its words and every tag, and a filtered
//! view leaves out the headings with nothing left under them, as Settings
//! does (`views::shown`). Enter on a workspace key presses it in the pane
//! mission control is over, as an M-p pick is pressed, but for the diff's
//! own commands (`#diffview`), which only the diff takes, and an app's
//! own keys, which only that app takes.
//!
//! The tmux version read tmux's bindings with `palette.sh --list` in the
//! background as the daemon started and each time the view was come to;
//! here they are the server's own bindings (`keys.json` included), so the
//! view is built from what the server holds, reading nothing and starting
//! nothing.

use std::collections::{BTreeMap, HashMap};

use serde_json::{Value, json};

use super::views::header;
use crate::items::{Item, State};
use crate::mux::input::Key;
use crate::mux::keys::{Action, Bindings};
use crate::ui::palette;

/// What the view reads besides the bindings: the settings that take keys
/// out of it.
#[derive(Clone, Copy, Debug)]
pub struct Shown {
    pub github: bool,
    pub linear: bool,
    /// `ga` and its boxes, in the diff and the palette (diff_ask).
    pub ask: bool,
    /// Easy mode's Cmd keys in Notes (easy_keys).
    pub easy: bool,
}

/// Which part of the list a group is in, which the board's panel puts
/// under a label each.
pub const WORKSPACE: &str = "workspace";
pub const APPS: &str = "apps";
pub const THROUGH: &str = "through";

/// One key's row, before it is an item.
struct Row<'a> {
    id: String,
    keys: String,
    notations: Vec<String>,
    does: &'a str,
    tags: &'a str,
    press: Option<String>,
    action: Option<String>,
    group: &'a str,
    count: u64,
    yours: bool,
    app: Option<&'a str>,
}

/// One key's row: what it does, the key, and its tags, the action to press
/// when it can be pressed from here, its group, and how often it was
/// pressed this week.
fn row(r: Row) -> Item {
    // The habits count only keys pressed alone (src/mux/server.rs).
    let counted = r.notations.iter().any(|k| !k.contains(' '));
    Item::new(r.id, r.does)
        .detail(r.tags)
        .state(State::Idle)
        .data(json!({
            "key": r.keys,
            "keys": r.notations,
            "press": r.press,
            "action": r.action,
            "search": format!("{} {}", r.keys, r.group),
            "group": r.group,
            "count": r.count,
            "counted": counted,
            "yours": r.yours,
            "app": r.app,
        }))
}

/// A group's heading, with the part of the list it is in.
fn heading(group: &str, count: usize, section: &str) -> Item {
    let mut h = header(&format!("keys-{group}"), group, count);
    h.data["section"] = json!(section);
    h.data["group"] = json!(group);
    h
}

/// The keys pressed alone that are bound to `action`, in tmux's notation.
fn keys_of(bindings: &Bindings, action: &Action) -> Vec<String> {
    let mut root: Vec<String> = bindings
        .root
        .iter()
        .filter(|(_, a)| *a == action)
        .map(|(k, _)| k.notation())
        .collect();
    root.sort();
    root
}

/// Whether `keys.json` bound a key, written as the palette writes it (a
/// prefix key after the prefix).
fn is_yours(bindings: &Bindings, notation: &str) -> bool {
    match notation.split_once(' ') {
        Some((_, after)) => Key::parse(after).is_some_and(|k| bindings.yours_prefix.contains(&k)),
        None => Key::parse(notation).is_some_and(|k| bindings.yours.contains(&k)),
    }
}

/// The letter or digit an Alt key is on the keyboard (`M-j` and `M-J`
/// both J), for the heatmap; `None` for any other key.
pub fn alt_letter(notation: &str) -> Option<char> {
    let rest = notation.strip_prefix("M-")?;
    let rest = rest.strip_prefix("S-").unwrap_or(rest);
    let mut chars = rest.chars();
    let c = chars.next()?;
    (chars.next().is_none() && c.is_ascii_alphanumeric()).then(|| c.to_ascii_uppercase())
}

/// How often each Alt key was pressed this week, by its letter.
pub fn heat(counts: &HashMap<String, u64>) -> BTreeMap<char, u64> {
    let mut out = BTreeMap::new();
    for (notation, n) in counts {
        if let Some(c) = alt_letter(notation) {
            *out.entry(c).or_insert(0) += n;
        }
    }
    out
}

/// The view's head: the heatmap's counts and whether they are counted.
pub fn head(counts: &HashMap<String, u64>, habits: bool) -> Value {
    let heat: serde_json::Map<String, Value> = heat(counts)
        .into_iter()
        .map(|(c, n)| (c.to_string(), json!(n)))
        .collect();
    json!({ "heat": heat, "habits": habits })
}

/// Every key a row stands for, root and after the prefix, for its count
/// (the habits counting only the root's), its "yours", the heatmap's
/// outline and its clicks: the palette shows a
/// prefix key only when no key pressed alone does the same, and lists M-1
/// for M-1 to M-9, whose row stands for tabs 2 to 9 too.
fn shared_keys(bindings: &Bindings, e: &palette::Entry) -> Vec<String> {
    let mut actions = vec![e.action.clone()];
    if e.action == Action::SelectTab(1) {
        actions.extend((2..=9).map(Action::SelectTab));
    }
    let prefix = bindings.prefix_key.notation();
    let mut out = e.keys.clone();
    for action in &actions {
        let mut more = keys_of(bindings, action);
        let mut after: Vec<String> = bindings
            .prefix
            .iter()
            .filter(|(_, a)| *a == action)
            .map(|(k, _)| format!("{prefix} {}", k.notation()))
            .collect();
        after.sort();
        more.extend(after);
        for k in more {
            if !out.contains(&k) {
                out.push(k);
            }
        }
    }
    out
}

/// The workspace's keys, grouped by their first tag, the biggest group
/// first, each group's in the order of what they do; then the commands no
/// key is bound to, under "Through M-p".
fn workspace(bindings: &Bindings, shown: Shown, counts: &HashMap<String, u64>) -> Vec<Item> {
    let mut entries = palette::entries(bindings, shown.github, shown.linear, &[]);
    // M-p itself, which the palette leaves out of its own list.
    if let Some((words, tags)) = Action::Palette.palette_note() {
        entries.push(palette::Entry {
            action: Action::Palette,
            words,
            tags,
            keys: keys_of(bindings, &Action::Palette),
        });
    }
    entries.retain(|e| shown.ask || e.action != Action::Diff(crate::diff::Command::Ask));
    // A command no key is bound to is reached through the palette.
    let through = keys_of(bindings, &Action::Palette)
        .first()
        .map_or_else(|| "M-p".into(), |k| palette::shown_key(k));
    let through_group = format!("Through {through}");
    let mut groups: Vec<(String, Vec<Item>)> = Vec::new();
    let mut keyless: Vec<Item> = Vec::new();
    for (n, e) in entries.iter().enumerate() {
        let tag_group = e
            .tags
            .split_whitespace()
            .next()
            .map_or("other", |t| t.trim_start_matches('#'))
            .to_string();
        let group = if e.keys.is_empty() {
            through_group.clone()
        } else {
            tag_group.clone()
        };
        let keys = if e.keys.is_empty() {
            through.clone()
        } else {
            e.keys
                .iter()
                .map(|k| palette::shown_key(k))
                .collect::<Vec<_>>()
                .join(", ")
        };
        // The diff's own commands type into the diff, so only it takes them.
        let press = (!e.tags.split_whitespace().any(|t| t == "#diffview")).then(|| e.action.name());
        let notations = shared_keys(bindings, e);
        let item = row(Row {
            id: format!("key:{n}"),
            keys,
            notations: notations.clone(),
            does: &e.words,
            tags: &e.tags,
            press,
            action: Some(e.action.name()),
            group: &group,
            count: notations
                .iter()
                .map(|k| counts.get(k).copied().unwrap_or(0))
                .sum(),
            yours: notations.iter().any(|k| is_yours(bindings, k)),
            app: None,
        });
        // The tag stays findable by the filter in "Through M-p".
        let mut item = item;
        if e.keys.is_empty() {
            item.data["search"] = json!(format!("{through} {tag_group}"));
            keyless.push(item);
            continue;
        }
        match groups.iter_mut().find(|(g, _)| *g == group) {
            Some((_, items)) => items.push(item),
            None => groups.push((group, vec![item])),
        }
    }
    groups.sort_by(|(a, x), (b, y)| y.len().cmp(&x.len()).then_with(|| a.cmp(b)));
    let mut out = Vec::new();
    for (group, mut items) in groups {
        items.sort_by_key(|a| a.title.to_lowercase());
        out.push(heading(&group, items.len(), WORKSPACE));
        out.extend(items);
    }
    if !keyless.is_empty() {
        keyless.sort_by_key(|a| a.title.to_lowercase());
        out.push(heading(&through_group, keyless.len(), THROUGH));
        out.extend(keyless);
    }
    out
}

/// A part's own keys under its heading.
fn part(title: &str, rows: &[(String, String)]) -> Vec<Item> {
    let mut out = vec![heading(title, rows.len(), APPS)];
    out.extend(rows.iter().enumerate().map(|(n, (keys, does))| {
        row(Row {
            id: format!("{title}:{n}"),
            keys: keys.clone(),
            notations: Vec::new(),
            does,
            tags: "",
            press: None,
            action: None,
            group: title,
            count: 0,
            yours: false,
            app: Some(title),
        })
    }));
    out
}

/// The view's rows: the workspace's keys, then the sidebar's, mission
/// control's, the diff's and Notes', then those only through M-p, with
/// `counts` the keys pressed this week by their notation.
pub fn items(bindings: &Bindings, shown: Shown, counts: &HashMap<String, u64>) -> Vec<Item> {
    let owned = |rows: &[(&str, &str)]| -> Vec<(String, String)> {
        rows.iter()
            .map(|(k, d)| (k.to_string(), d.to_string()))
            .collect()
    };
    let mut all = workspace(bindings, shown, counts);
    let through_at = all
        .iter()
        .position(|i| i.data["section"] == THROUGH)
        .unwrap_or(all.len());
    let through = all.split_off(through_at);
    let mut out = all;
    out.extend(part("Sidebar", &owned(crate::sidebar::KEYS)));
    out.extend(part("Mission control", &owned(super::serve::KEYS)));
    for (title, rows) in crate::diff::app::key_groups(shown.ask) {
        out.extend(part(title, &rows));
    }
    for (title, rows) in crate::notes::app::key_groups(shown.easy) {
        out.extend(part(title, &rows));
    }
    out.extend(through);
    out
}

/// The key a row is for, as the view shows it.
pub fn key_of(item: &Item) -> Option<&str> {
    item.data.get("key").and_then(serde_json::Value::as_str)
}

/// The action Enter presses for a row: a workspace key's.
pub fn press_of(item: &Item) -> Option<Action> {
    item.data
        .get("press")
        .and_then(serde_json::Value::as_str)
        .and_then(Action::parse)
}

/// The workspace action a row is for, pressed from here or not.
pub fn action_of(item: &Item) -> Option<Action> {
    item.data
        .get("action")
        .and_then(Value::as_str)
        .and_then(Action::parse)
}

/// The group a row or a heading is in.
pub fn group_of(item: &Item) -> &str {
    item.data.get("group").and_then(Value::as_str).unwrap_or("")
}

/// The app whose own key a row is, which only that app takes.
pub fn app_of(item: &Item) -> Option<&str> {
    item.data.get("app").and_then(Value::as_str)
}

/// How often a row's keys were pressed this week; 0 for a heading, whose
/// `count` is its rows'.
pub fn count_of(item: &Item) -> u64 {
    if super::views::is_header(item) {
        return 0;
    }
    item.data.get("count").and_then(Value::as_u64).unwrap_or(0)
}

/// Whether the habits count a row's presses: it has a key pressed alone.
pub fn counted(item: &Item) -> bool {
    item.data.get("counted").and_then(Value::as_bool) == Some(true)
}

/// Whether `keys.json` bound a row's key.
pub fn yours_of(item: &Item) -> bool {
    item.data.get("yours").and_then(Value::as_bool) == Some(true)
}

/// What Enter on a row of an app's own says: only that app takes it.
pub fn only_there(item: &Item) -> String {
    let key = key_of(item).unwrap_or("it");
    match app_of(item) {
        Some(app) => format!("{key} is {}'s: press it there.", app_name(app)),
        None => format!("{key} is the diff's: press it there."),
    }
}

/// An app's name as a sentence says it: "the sidebar", "mission control".
fn app_name(app: &str) -> String {
    match app {
        "Sidebar" => "the sidebar".into(),
        "Mission control" => "mission control".into(),
        a if a.starts_with("Notes") => "Notes".into(),
        a if a.starts_with("M-d") => "the diff".into(),
        a => a.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::ViewData;
    use crate::mission::views::{self, View, ViewId, is_header};

    const ALL: Shown = Shown {
        github: true,
        linear: true,
        ask: true,
        easy: false,
    };

    fn titles(items: &[Item]) -> Vec<&str> {
        items.iter().map(|i| i.title.as_str()).collect()
    }

    #[test]
    fn the_workspaces_keys_come_first_by_their_first_tag_the_biggest_group_first() {
        let items = items(&Bindings::default(), ALL, &HashMap::new());
        assert!(is_header(&items[0]));
        let counts: Vec<u64> = items
            .iter()
            .take_while(|i| i.title != "Sidebar")
            .filter(|i| is_header(i))
            .map(|i| i.data["count"].as_u64().unwrap())
            .collect();
        assert!(counts.windows(2).all(|w| w[0] >= w[1]), "{counts:?}");
        // Split pane to the left, under panes, M-H shown as M-S-H.
        let split = items
            .iter()
            .find(|i| i.title == "Split pane to the left")
            .unwrap();
        assert_eq!(key_of(split), Some("M-S-H"));
        assert_eq!(split.detail, "#panes #split");
        assert_eq!(
            press_of(split),
            Some(Action::Split(crate::mux::Direction::Left))
        );
        // M-p itself is listed, and a command with no key is reached
        // through it.
        let palette = items
            .iter()
            .find(|i| press_of(i) == Some(Action::Palette))
            .unwrap();
        assert_eq!(key_of(palette), Some("M-p"));
        let theme = items
            .iter()
            .find(|i| press_of(i) == Some(Action::ToggleTheme))
            .unwrap();
        assert_eq!(key_of(theme), Some("M-p"));
        // A prefix key is written after the prefix.
        let detach = items
            .iter()
            .find(|i| press_of(i) == Some(Action::Detach))
            .unwrap();
        assert_eq!(key_of(detach), Some("M-b d"));
    }

    #[test]
    fn the_diffs_commands_are_listed_but_not_pressed_and_ga_goes_with_diff_ask() {
        let all = items(&Bindings::default(), ALL, &HashMap::new());
        let ask = all
            .iter()
            .find(|i| i.title.starts_with("Ask Claude about the diff"))
            .unwrap();
        assert_eq!(press_of(ask), None, "only the diff takes it");
        assert!(titles(&all).contains(&"M-d: an answer (ga)"));
        let without = items(
            &Bindings::default(),
            Shown { ask: false, ..ALL },
            &HashMap::new(),
        );
        assert!(
            !without
                .iter()
                .any(|i| i.title.starts_with("Ask Claude about the diff"))
        );
        assert!(!titles(&without).contains(&"M-d: an answer (ga)"));
        // Notes' own ga, Add an item, stays.
        assert!(
            !without
                .iter()
                .any(|i| key_of(i) == Some("ga") && !i.title.contains("Add an item"))
        );
    }

    #[test]
    fn the_parts_own_keys_follow_with_notes_leader_keys_and_cmd_keys_in_easy_mode() {
        let all = items(&Bindings::default(), ALL, &HashMap::new());
        let names = titles(&all);
        let at = |t: &str| names.iter().position(|n| *n == t).unwrap();
        assert!(at("Sidebar") < at("Mission control"));
        assert!(at("Mission control") < at("M-d's diff"));
        assert!(at("M-d's diff") < at("Notes: a note"));
        assert!(at("Notes: a note") < at("Notes: the list"));
        let bold = all.iter().find(|i| key_of(i) == Some("Space b")).unwrap();
        assert!(bold.title.starts_with("Bold"));
        assert!(!all.iter().any(|i| key_of(i) == Some("⌘S")));
        let easy = items(
            &Bindings::default(),
            Shown { easy: true, ..ALL },
            &HashMap::new(),
        );
        assert!(easy.iter().any(|i| key_of(i) == Some("⌘S")));
        // The workspace's GitHub keys go with GitHub.
        let off = items(
            &Bindings::default(),
            Shown {
                github: false,
                ..ALL
            },
            &HashMap::new(),
        );
        assert!(!off.iter().any(|i| i.detail.contains("#github")));
    }

    #[test]
    fn the_groups_go_workspace_then_the_apps_then_through_m_p_with_the_keyless_there() {
        let all = items(&Bindings::default(), ALL, &HashMap::new());
        let sections: Vec<&str> = all
            .iter()
            .filter(|i| is_header(i))
            .map(|i| i.data["section"].as_str().unwrap())
            .collect();
        let first_app = sections.iter().position(|s| *s == APPS).unwrap();
        assert!(sections[..first_app].iter().all(|s| *s == WORKSPACE));
        assert_eq!(sections.last(), Some(&THROUGH));
        assert_eq!(sections.iter().filter(|s| **s == THROUGH).count(), 1);
        let theme = all
            .iter()
            .find(|i| press_of(i) == Some(Action::ToggleTheme))
            .unwrap();
        assert_eq!(group_of(theme), "Through M-p");
        assert!(
            all.iter()
                .filter(|i| !is_header(i) && app_of(i).is_none() && group_of(i) != "Through M-p")
                .all(|i| key_of(i) != Some("M-p") || press_of(i) == Some(Action::Palette))
        );
    }

    #[test]
    fn the_weeks_presses_sum_by_row_and_alt_letter_and_keys_json_marks_yours() {
        let counts: HashMap<String, u64> = [("M-j", 5), ("M-J", 2), ("M-b d", 1), ("M-1", 3)]
            .into_iter()
            .map(|(k, n)| (k.to_string(), n))
            .collect();
        let heat = heat(&counts);
        assert_eq!(heat.get(&'J'), Some(&7));
        assert_eq!(heat.get(&'1'), Some(&3));
        assert_eq!(heat.len(), 2, "a prefix key isn't on the heatmap");
        let mut b = Bindings::default();
        b.apply(&json!({ "root": { "M-g": "new-tab" } }));
        let all = items(&b, ALL, &counts);
        let find = all
            .iter()
            .find(|i| press_of(i) == Some(Action::FindTab))
            .unwrap();
        assert_eq!(count_of(find), 5);
        assert!(!yours_of(find));
        let new_tab = all
            .iter()
            .find(|i| press_of(i) == Some(Action::NewTab))
            .unwrap();
        assert!(yours_of(new_tab));
        // M-1's row stands for M-1 to M-9.
        let by_number = all
            .iter()
            .find(|i| press_of(i) == Some(Action::SelectTab(1)))
            .unwrap();
        let counts4: HashMap<String, u64> = [("M-4".to_string(), 6)].into_iter().collect();
        let again = items(&Bindings::default(), ALL, &counts4);
        let row = again.iter().find(|i| i.id == by_number.id).unwrap();
        assert_eq!(count_of(row), 6);
        assert!(
            row.data["keys"]
                .as_array()
                .unwrap()
                .iter()
                .any(|k| k == "M-9")
        );
        // A prefix key keys.json binds to an action with a root key too
        // makes that row yours, and counts its presses.
        let mut b = Bindings::default();
        b.apply(&json!({ "prefix": { "x": "find-tab" } }));
        let presses: HashMap<String, u64> = [("M-b x".to_string(), 2)].into_iter().collect();
        let all = items(&b, ALL, &presses);
        let find = all
            .iter()
            .find(|i| press_of(i) == Some(Action::FindTab))
            .unwrap();
        assert!(yours_of(find));
        assert_eq!(count_of(find), 2);
        assert_eq!(count_of(&all[0]), 0, "a heading's count is its rows'");
        assert_eq!(alt_letter("M-S-h"), Some('H'));
        assert_eq!(alt_letter("M-Left"), None);
    }

    #[test]
    fn a_filter_matches_a_key_or_a_tag_and_leaves_out_the_empty_headings() {
        let view = View {
            id: ViewId::Keys,
            title: "Keys".into(),
            data: ViewData {
                items: items(&Bindings::default(), ALL, &HashMap::new()),
                ..ViewData::default()
            },
            loaded: true,
        };
        let by_key = views::shown(&view, "m-s-h", None, &views::Folds::default());
        let names = titles(&by_key.data.items);
        assert_eq!(
            names,
            [
                "panes",
                "Move the pane's divider left",
                "Move the pane's divider left by 1",
                "Split pane to the left",
                "Mission control",
                super::super::serve::KEYS
                    .iter()
                    .find(|(k, _)| k.starts_with("C-M-S-h"))
                    .unwrap()
                    .1,
            ]
        );
        let by_tag = views::shown(&view, "#split", None, &views::Folds::default());
        assert!(by_tag.data.items.iter().filter(|i| !is_header(i)).count() >= 4);
        assert!(
            by_tag
                .data
                .items
                .iter()
                .all(|i| is_header(i) || i.detail.contains("#split"))
        );
    }
}
