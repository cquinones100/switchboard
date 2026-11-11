//! Mission control's Keys view (M-/; lib/sidebar/keymap.js and
//! lib/sidebar/app-keys.json): every key Switchboard binds, in one list
//! under headings. First the workspace's, as the M-p palette lists them
//! (`ui::palette::entries`, M-p itself included), each under the first of
//! its tags, the biggest group first, a command no key is bound to shown as
//! reached through M-p; then the sidebar's and mission control's own (the
//! lists their `?` shows), M-d's diff's (`diff::app::KEYS`, the cover, the
//! box `ga` opens and its answer, those only with diff_ask) and Notes'
//! (`notes::app::key_groups`: a note's, with its leader keys and, while
//! easy_keys is on, easy mode's Cmd keys, then the list's), each from the
//! table its own code keeps. A key column as wide as the widest key cuts
//! none (`render`).
//!
//! Its `/` filter matches a key, its words and every tag, and a filtered
//! view leaves out the headings with nothing left under them, as Settings
//! does (`views::shown`). Enter on a workspace key offers to press it in
//! the pane mission control is over, as an M-p pick is pressed, but for the
//! diff's own commands (`#diffview`), which only the diff takes.
//!
//! The tmux version read tmux's bindings with `palette.sh --list` in the
//! background as the daemon started and each time the view was come to;
//! here they are the server's own bindings (`keys.json` included), so the
//! view is built from what the server holds, reading nothing and starting
//! nothing.

use serde_json::json;

use super::views::header;
use crate::items::{Item, State};
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

/// One key's row: what it does, the key, and its tags, the action to press
/// when it can be pressed from here.
fn row(id: String, keys: &str, does: &str, tags: &str, press: Option<String>, group: &str) -> Item {
    Item::new(id, does)
        .detail(tags)
        .state(State::Idle)
        .data(json!({
            "key": keys,
            "press": press,
            "search": format!("{keys} {group}"),
        }))
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

/// The workspace's keys, grouped by their first tag, the biggest group
/// first, each group's in the order of what they do.
fn workspace(bindings: &Bindings, shown: Shown) -> Vec<Item> {
    let mut entries = palette::entries(bindings, shown.github, shown.linear);
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
    let mut groups: Vec<(String, Vec<Item>)> = Vec::new();
    for (n, e) in entries.iter().enumerate() {
        let group = e
            .tags
            .split_whitespace()
            .next()
            .map_or("other", |t| t.trim_start_matches('#'))
            .to_string();
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
        let item = row(format!("key:{n}"), &keys, &e.words, &e.tags, press, &group);
        match groups.iter_mut().find(|(g, _)| *g == group) {
            Some((_, items)) => items.push(item),
            None => groups.push((group, vec![item])),
        }
    }
    groups.sort_by(|(a, x), (b, y)| y.len().cmp(&x.len()).then_with(|| a.cmp(b)));
    let mut out = Vec::new();
    for (group, mut items) in groups {
        items.sort_by_key(|a| a.title.to_lowercase());
        out.push(header(&format!("keys-{group}"), &group, items.len()));
        out.extend(items);
    }
    out
}

/// A part's own keys under its heading.
fn part(title: &str, rows: &[(String, String)]) -> Vec<Item> {
    let mut out = vec![header(&format!("keys-{title}"), title, rows.len())];
    out.extend(
        rows.iter()
            .enumerate()
            .map(|(n, (keys, does))| row(format!("{title}:{n}"), keys, does, "", None, title)),
    );
    out
}

/// The view's rows: the workspace's keys, then the sidebar's, mission
/// control's, the diff's and Notes'.
pub fn items(bindings: &Bindings, shown: Shown) -> Vec<Item> {
    let owned = |rows: &[(&str, &str)]| -> Vec<(String, String)> {
        rows.iter()
            .map(|(k, d)| (k.to_string(), d.to_string()))
            .collect()
    };
    let mut out = workspace(bindings, shown);
    out.extend(part("Sidebar", &owned(crate::sidebar::KEYS)));
    out.extend(part("Mission control", &owned(super::serve::KEYS)));
    for (title, rows) in crate::diff::app::key_groups(shown.ask) {
        out.extend(part(title, &rows));
    }
    for (title, rows) in crate::notes::app::key_groups(shown.easy) {
        out.extend(part(title, &rows));
    }
    out
}

/// The key a row is for, as the view shows it.
pub fn key_of(item: &Item) -> Option<&str> {
    item.data.get("key").and_then(serde_json::Value::as_str)
}

/// The action Enter offers to press for a row: a workspace key's.
pub fn press_of(item: &Item) -> Option<Action> {
    item.data
        .get("press")
        .and_then(serde_json::Value::as_str)
        .and_then(Action::parse)
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
        let items = items(&Bindings::default(), ALL);
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
        let all = items(&Bindings::default(), ALL);
        let ask = all
            .iter()
            .find(|i| i.title.starts_with("Ask Claude about the diff"))
            .unwrap();
        assert_eq!(press_of(ask), None, "only the diff takes it");
        assert!(titles(&all).contains(&"M-d: an answer (ga)"));
        let without = items(&Bindings::default(), Shown { ask: false, ..ALL });
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
        let all = items(&Bindings::default(), ALL);
        let names = titles(&all);
        let at = |t: &str| names.iter().position(|n| *n == t).unwrap();
        assert!(at("Sidebar") < at("Mission control"));
        assert!(at("Mission control") < at("M-d's diff"));
        assert!(at("M-d's diff") < at("Notes: a note"));
        assert!(at("Notes: a note") < at("Notes: the list"));
        let bold = all.iter().find(|i| key_of(i) == Some("Space b")).unwrap();
        assert!(bold.title.starts_with("Bold"));
        assert!(!all.iter().any(|i| key_of(i) == Some("⌘S")));
        let easy = items(&Bindings::default(), Shown { easy: true, ..ALL });
        assert!(easy.iter().any(|i| key_of(i) == Some("⌘S")));
        // The workspace's GitHub keys go with GitHub.
        let off = items(
            &Bindings::default(),
            Shown {
                github: false,
                ..ALL
            },
        );
        assert!(!off.iter().any(|i| i.detail.contains("#github")));
    }

    #[test]
    fn a_filter_matches_a_key_or_a_tag_and_leaves_out_the_empty_headings() {
        let view = View {
            id: ViewId::Keys,
            title: "Keys".into(),
            data: ViewData {
                items: items(&Bindings::default(), ALL),
                ..ViewData::default()
            },
            loaded: true,
        };
        let by_key = views::shown(&view, "m-s-h", None, &views::Folds::default());
        let names = titles(&by_key.data.items);
        assert_eq!(names, ["panes", "Split pane to the left"]);
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
