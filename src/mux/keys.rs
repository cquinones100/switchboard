//! Key bindings: which key does what. The defaults are the multiplexer keys
//! tmux.conf bound (moving between panes and tabs, splitting, closing,
//! zooming, resizing) and its Claude keys (M-a to what needs you, M-c to the
//! tab's Claude, M-f to pin a tab), under the same keys, with the prefix
//! `M-b` for the rest; `~/.config/switchboard/keys.json` adds to or changes them:
//!
//! ```json
//! { "root": { "M-g": "new-tab", "M-w": null }, "prefix": { "y": "copy-mode" } }
//! ```
//!
//! A binding names an action, as `switchboard help-keys` lists them, or
//! `null` to unbind. Each action is one of `Action`'s, written as it is
//! parsed by `Action::parse`.
//!
//! Each action has a description and tags (`Action::note`), as tmux.conf
//! gave every binding one with `bind -N "Split pane to the left #panes
//! #split"`: the M-p palette (src/ui/palette.rs) lists every action by it,
//! with its keys, and the actions no key is bound to by default
//! (`Action::palette_only`) with none.

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
    /// The next tab that needs you: pinned, then waiting, then done.
    NextNeedsYou,
    /// Into the pane running Claude in this tab.
    FocusClaude,
    /// Pin the tab, keeping it at the top of what needs you, or unpin it.
    TogglePin,
    /// The palette: every command, to find and run one (M-p).
    Palette,
    /// Show every tip again, each once more.
    ResetTips,
}

impl Action {
    /// An action from its name in keys.json: `select-pane-left`,
    /// `prev-tab`, `next-tab`, `last-tab`, `select-tab-3`, `new-tab`,
    /// `split-right`, `close-pane`, `zoom`, `resize-left-5`, `detach`,
    /// `copy-mode`, `send-prefix`, `list-keys`, `next-needs-you`,
    /// `focus-claude`, `toggle-pin`, `palette`, `reset-tips`.
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
            "next-needs-you" => Action::NextNeedsYou,
            "focus-claude" => Action::FocusClaude,
            "toggle-pin" => Action::TogglePin,
            "palette" => Action::Palette,
            "reset-tips" => Action::ResetTips,
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
            Action::NextNeedsYou => "next-needs-you".into(),
            Action::FocusClaude => "focus-claude".into(),
            Action::TogglePin => "toggle-pin".into(),
            Action::Palette => "palette".into(),
            Action::ResetTips => "reset-tips".into(),
        }
    }

    /// What it does, in words, for the list of keys: tmux.conf's
    /// descriptions without their tags.
    pub fn about(&self) -> String {
        let side = |d: &Direction| match d {
            Direction::Left => "to the left",
            Direction::Right => "to the right",
            Direction::Up => "above",
            Direction::Down => "below",
        };
        let toward = |d: &Direction| match d {
            Direction::Left => "left",
            Direction::Right => "right",
            Direction::Up => "up",
            Direction::Down => "down",
        };
        match self {
            Action::SelectPane(Direction::Left) => "Move to the pane on the left".into(),
            Action::SelectPane(Direction::Right) => "Move to the pane on the right".into(),
            Action::SelectPane(d) => format!("Move to the pane {}", side(d)),
            Action::PrevTab => "Previous tab".into(),
            Action::NextTab => "Next tab".into(),
            Action::LastTab => "Go back to the previous tab".into(),
            Action::SelectTab(n) => format!("Go to tab {n}"),
            Action::NewTab => "New tab".into(),
            Action::Split(d) => format!("Split pane {}", side(d)),
            Action::ClosePane => "Close the pane".into(),
            Action::Zoom => "Zoom the pane in or out".into(),
            Action::Resize(d, 5) => format!("Grow pane {}", toward(d)),
            Action::Resize(d, n) => format!("Grow pane {} by {n}", toward(d)),
            Action::Detach => "Detach, leaving everything running".into(),
            Action::CopyMode => "Scroll back through the pane's history".into(),
            Action::SendPrefix => "Send the prefix key to the pane".into(),
            Action::ListKeys => "List the keys".into(),
            Action::NextNeedsYou => "Go to the next tab that needs you".into(),
            Action::FocusClaude => "Jump to the pane running Claude in this tab".into(),
            Action::TogglePin => "Pin or unpin this tab, keeping it at the top of Needs you".into(),
            Action::Palette => "Find a command and run it (the palette)".into(),
            Action::ResetTips => "Show the tips again, each once more".into(),
        }
    }

    /// What it is about, as tmux.conf's `#tags`: matched in the palette as
    /// any other word, and shown dimmed.
    pub fn tags(&self) -> &'static [&'static str] {
        match self {
            Action::SelectPane(_) => &["panes", "navigate"],
            Action::PrevTab | Action::NextTab | Action::LastTab | Action::SelectTab(_) => {
                &["tabs", "navigate"]
            }
            Action::NewTab => &["tabs", "new"],
            Action::Split(_) => &["panes", "split"],
            Action::ClosePane => &["panes", "close"],
            Action::Zoom => &["panes", "layout"],
            Action::Resize(..) => &["panes", "resize"],
            Action::Detach => &["session", "detach"],
            Action::CopyMode => &["panes", "scroll", "history"],
            Action::SendPrefix => &["keys"],
            Action::ListKeys => &["help", "keys"],
            Action::NextNeedsYou | Action::FocusClaude => &["claude", "tabs", "navigate"],
            Action::TogglePin => &["tabs", "pin"],
            Action::Palette => &["palette"],
            Action::ResetTips => &["help", "tips"],
        }
    }

    /// Its description as tmux.conf wrote one: the words, then the tags
    /// (`Split pane to the left #panes #split`).
    pub fn note(&self) -> String {
        let mut note = self.about();
        for tag in self.tags() {
            note.push_str(" #");
            note.push_str(tag);
        }
        note
    }

    /// The words and tags the palette lists it by, or `None` to leave it
    /// out: of M-1 to M-9, only the first, for all nine, as tmux.conf gave
    /// only M-1 a description.
    pub fn palette_note(&self) -> Option<(String, String)> {
        let words = match self {
            Action::SelectTab(1) => "Go to a tab by number (M-1 to M-9)".to_string(),
            Action::SelectTab(_) => return None,
            _ => self.about(),
        };
        let tags = self
            .tags()
            .iter()
            .map(|t| format!("#{t}"))
            .collect::<Vec<_>>()
            .join(" ");
        Some((words, tags))
    }

    /// The commands no key is bound to by default, which the palette lists
    /// with none (tmux.conf's `palette` key table).
    pub fn palette_only() -> Vec<Action> {
        vec![Action::ResetTips]
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
        bind("M-a", NextNeedsYou);
        bind("M-c", FocusClaude);
        bind("M-f", TogglePin);
        bind("M-p", Palette);
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
            Action::NextNeedsYou,
            Action::FocusClaude,
            Action::TogglePin,
            Action::Palette,
            Action::ResetTips,
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

    #[test]
    fn notes_read_as_tmux_confs() {
        assert_eq!(
            Action::Split(Direction::Left).note(),
            "Split pane to the left #panes #split"
        );
        assert_eq!(
            Action::Resize(Direction::Down, 5).note(),
            "Grow pane down #panes #resize"
        );
        assert_eq!(
            Action::Resize(Direction::Up, 2).about(),
            "Grow pane up by 2"
        );
        assert_eq!(
            Action::SelectTab(1).palette_note().unwrap().0,
            "Go to a tab by number (M-1 to M-9)"
        );
        assert_eq!(Action::SelectTab(4).palette_note(), None);
    }
}
