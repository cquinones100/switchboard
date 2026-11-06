//! Key bindings: which key does what. The defaults are the multiplexer keys
//! tmux.conf bound (moving between panes and tabs, splitting, closing,
//! zooming, resizing), under the same keys, with the prefix `M-b` for the
//! rest; `~/.config/switchboard/keys.json` adds to or changes them:
//!
//! ```json
//! { "root": { "M-g": "new-tab", "M-w": null }, "prefix": { "y": "copy-mode" } }
//! ```
//!
//! A binding names an action, as `switchboard help-keys` lists them, or
//! `null` to unbind. Each action is one of `Action`'s, written as it is
//! parsed by `Action::parse`.

use std::collections::HashMap;

use serde_json::Value;

use super::Direction;
use super::input::Key;

/// What a key does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Move to the pane in a direction.
    SelectPane(Direction),
    /// The previous or next tab, wrapping round.
    PrevTab,
    NextTab,
    /// The tab you were on before.
    LastTab,
    /// The tab with this number (1-based, as the status line shows).
    SelectTab(usize),
    /// A new tab, its shell in the current pane's directory.
    NewTab,
    /// A new pane on that side of the current one, in its directory.
    Split(Direction),
    /// Close the current pane (its tab, when it is the last).
    ClosePane,
    /// Zoom the current pane to the whole tab, or back.
    Zoom,
    /// Grow the current pane toward a direction by cells.
    Resize(Direction, u16),
    /// Leave the workspace running and end this client.
    Detach,
    /// Scroll back through the pane's history, with vi keys.
    CopyMode,
    /// The prefix key itself, sent to the pane.
    SendPrefix,
    /// Every binding, in a list.
    ListKeys,
}

impl Action {
    /// An action from its name in keys.json: `select-pane-left`,
    /// `prev-tab`, `next-tab`, `last-tab`, `select-tab-3`, `new-tab`,
    /// `split-right`, `close-pane`, `zoom`, `resize-left-5`, `detach`,
    /// `copy-mode`, `send-prefix`, `list-keys`.
    pub fn parse(name: &str) -> Option<Action> {
        let dir = |d: &str| match d {
            "left" => Some(Direction::Left),
            "right" => Some(Direction::Right),
            "up" => Some(Direction::Up),
            "down" => Some(Direction::Down),
            _ => None,
        };
        Some(match name {
            "prev-tab" => Action::PrevTab,
            "next-tab" => Action::NextTab,
            "last-tab" => Action::LastTab,
            "new-tab" => Action::NewTab,
            "close-pane" => Action::ClosePane,
            "zoom" => Action::Zoom,
            "detach" => Action::Detach,
            "copy-mode" => Action::CopyMode,
            "send-prefix" => Action::SendPrefix,
            "list-keys" => Action::ListKeys,
            _ => {
                if let Some(d) = name.strip_prefix("select-pane-") {
                    Action::SelectPane(dir(d)?)
                } else if let Some(d) = name.strip_prefix("split-") {
                    Action::Split(dir(d)?)
                } else if let Some(n) = name.strip_prefix("select-tab-") {
                    Action::SelectTab(n.parse().ok().filter(|n| *n > 0)?)
                } else {
                    let rest = name.strip_prefix("resize-")?;
                    let (d, cells) = rest.split_once('-').unwrap_or((rest, "5"));
                    Action::Resize(dir(d)?, cells.parse().ok()?)
                }
            }
        })
    }

    /// The action's name, as `parse` reads it.
    pub fn name(&self) -> String {
        let dir = |d: &Direction| match d {
            Direction::Left => "left",
            Direction::Right => "right",
            Direction::Up => "up",
            Direction::Down => "down",
        };
        match self {
            Action::SelectPane(d) => format!("select-pane-{}", dir(d)),
            Action::PrevTab => "prev-tab".into(),
            Action::NextTab => "next-tab".into(),
            Action::LastTab => "last-tab".into(),
            Action::SelectTab(n) => format!("select-tab-{n}"),
            Action::NewTab => "new-tab".into(),
            Action::Split(d) => format!("split-{}", dir(d)),
            Action::ClosePane => "close-pane".into(),
            Action::Zoom => "zoom".into(),
            Action::Resize(d, n) => format!("resize-{}-{n}", dir(d)),
            Action::Detach => "detach".into(),
            Action::CopyMode => "copy-mode".into(),
            Action::SendPrefix => "send-prefix".into(),
            Action::ListKeys => "list-keys".into(),
        }
    }

    /// What it does, in words, for the list of keys.
    pub fn about(&self) -> String {
        let dir = |d: &Direction| match d {
            Direction::Left => "left",
            Direction::Right => "right",
            Direction::Up => "above",
            Direction::Down => "below",
        };
        match self {
            Action::SelectPane(d) => format!("Move to the pane {}", dir(d)),
            Action::PrevTab => "Previous tab".into(),
            Action::NextTab => "Next tab".into(),
            Action::LastTab => "Go back to the previous tab".into(),
            Action::SelectTab(n) => format!("Go to tab {n}"),
            Action::NewTab => "New tab".into(),
            Action::Split(d) => format!("Split pane {}", dir(d)),
            Action::ClosePane => "Close the pane".into(),
            Action::Zoom => "Zoom the pane in or out".into(),
            Action::Resize(d, n) => format!("Grow pane {} by {n}", dir(d)),
            Action::Detach => "Detach, leaving everything running".into(),
            Action::CopyMode => "Scroll back through the pane's history".into(),
            Action::SendPrefix => "Send the prefix key to the pane".into(),
            Action::ListKeys => "List the keys".into(),
        }
    }
}

/// The two tables of bindings: keys pressed alone (`root`), and keys pressed
/// after the prefix.
#[derive(Clone, Debug)]
pub struct Bindings {
    pub prefix_key: Key,
    pub root: HashMap<Key, Action>,
    pub prefix: HashMap<Key, Action>,
}

fn key(notation: &str) -> Key {
    Key::parse(notation)
        .unwrap_or_else(|| panic!("a default binding's key, {notation}, reads as a key"))
}

impl Default for Bindings {
    fn default() -> Bindings {
        use Action::*;
        use Direction::*;
        let mut root = HashMap::new();
        let mut bind = |k: &str, a: Action| {
            root.insert(key(k), a);
        };
        bind("M-Left", SelectPane(Left));
        bind("M-Right", SelectPane(Right));
        bind("M-Up", SelectPane(Up));
        bind("M-Down", SelectPane(Down));
        bind("M-[", PrevTab);
        bind("M-]", NextTab);
        bind("M-Tab", LastTab);
        for n in 1..=9 {
            bind(&format!("M-{n}"), SelectTab(n));
        }
        bind("M-H", Split(Left));
        bind("M-J", Split(Down));
        bind("M-K", Split(Up));
        bind("M-L", Split(Right));
        bind("M-w", ClosePane);
        bind("M-=", Zoom);
        bind("M-S-Left", Resize(Left, 5));
        bind("M-S-Right", Resize(Right, 5));
        bind("M-S-Up", Resize(Up, 5));
        bind("M-S-Down", Resize(Down, 5));
        bind("M-t", NewTab);
        bind("M-n", NewTab);
        let mut prefix = HashMap::new();
        let mut pbind = |k: &str, a: Action| {
            prefix.insert(key(k), a);
        };
        pbind("d", Detach);
        pbind("[", CopyMode);
        pbind("c", NewTab);
        pbind("x", ClosePane);
        pbind("z", Zoom);
        pbind("?", ListKeys);
        pbind("M-b", SendPrefix);
        pbind("%", Split(Right));
        pbind("\"", Split(Down));
        Bindings {
            prefix_key: key("M-b"),
            root,
            prefix,
        }
    }
}

impl Bindings {
    /// The defaults with `keys.json`'s changes, and the changes it couldn't
    /// read, in words.
    pub fn load() -> (Bindings, Vec<String>) {
        let mut bindings = Bindings::default();
        let file = crate::paths::config_dir().join("keys.json");
        let Some(text) = crate::util::read_text(&file) else {
            return (bindings, Vec::new());
        };
        let problems = match serde_json::from_str::<Value>(&text) {
            Ok(config) => bindings.apply(&config),
            Err(e) => vec![format!("{} isn't JSON: {e}", crate::paths::tilde(&file))],
        };
        (bindings, problems)
    }

    /// Applies a keys.json object; gives what it couldn't use.
    pub fn apply(&mut self, config: &Value) -> Vec<String> {
        let mut problems = Vec::new();
        if let Some(p) = config.get("prefix-key").and_then(Value::as_str) {
            match Key::parse(p) {
                Some(k) => self.prefix_key = k,
                None => problems.push(format!("prefix-key: {p} isn't a key")),
            }
        }
        for (table_name, table) in [("root", &mut self.root), ("prefix", &mut self.prefix)] {
            let Some(entries) = config.get(table_name).and_then(Value::as_object) else {
                continue;
            };
            for (notation, action) in entries {
                let Some(k) = Key::parse(notation) else {
                    problems.push(format!("{table_name}: {notation} isn't a key"));
                    continue;
                };
                match action {
                    Value::Null => {
                        table.remove(&k);
                    }
                    Value::String(name) => match Action::parse(name) {
                        Some(a) => {
                            table.insert(k, a);
                        }
                        None => problems.push(format!("{table_name}: {name} isn't an action")),
                    },
                    other => problems.push(format!(
                        "{table_name}: {notation}: {other} isn't an action name"
                    )),
                }
            }
        }
        problems
    }

    /// Every binding as (keys, what it does), the prefix's after the root's,
    /// each table in key order.
    pub fn list(&self) -> Vec<(String, String)> {
        let mut root: Vec<_> = self
            .root
            .iter()
            .map(|(k, a)| (k.notation(), a.about()))
            .collect();
        root.sort();
        let mut prefix: Vec<_> = self
            .prefix
            .iter()
            .map(|(k, a)| {
                (
                    format!("{} {}", self.prefix_key.notation(), k.notation()),
                    a.about(),
                )
            })
            .collect();
        prefix.sort();
        root.into_iter().chain(prefix).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_names_round_trip() {
        for a in [
            Action::SelectPane(Direction::Up),
            Action::SelectTab(7),
            Action::Resize(Direction::Left, 3),
            Action::Split(Direction::Down),
            Action::Detach,
        ] {
            assert_eq!(Action::parse(&a.name()), Some(a));
        }
        assert_eq!(
            Action::parse("resize-up"),
            Some(Action::Resize(Direction::Up, 5))
        );
        assert_eq!(Action::parse("select-tab-0"), None);
        assert_eq!(Action::parse("fly"), None);
    }
}
