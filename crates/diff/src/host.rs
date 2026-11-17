//! Who the diff asks for what isn't its own: the workspace, through the
//! plugin API (`switchboard_plugin_api::Host`), or, alone in a terminal,
//! itself (`Alone`). Every module asks through `host()`, which is whoever
//! ran the diff (`install`), so nothing here reaches the workspace another
//! way; the small modules at the end (`paths`, `settings`, `requests`)
//! give the answers the names the diff's modules knew them by.
//!
//! Alone, the diff has no server, no pull request and no list of whose
//! code a directory holds: it compares with the repository's default
//! branch (or the branch its own was cut from, when it tracks one), tells its story through `claude -p` with nothing changed in its
//! environment, counts no requests, and takes the repository you run it in
//! as yours. Its settings are its own defaults (the manifest's), over
//! which it reads Switchboard's `~/.config/switchboard/settings.json` when
//! you have one, so the theme and the options you set for the diff there
//! are the ones it follows; its state and cache are
//! `~/.local/state/switchboard-diff` and `~/.cache/switchboard-diff`.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use serde_json::Value;
use switchboard_plugin_api::sdk::home_join;
use switchboard_plugin_api::{Action, Answer, Folders, Host};

static HOST: OnceLock<Arc<dyn Host>> = OnceLock::new();

/// Says who answers for the workspace. The first one stays; with none
/// said (a test, a program that reads a change without running the diff),
/// the diff answers for itself with its defaults, reading no file of yours.
pub fn install(host: Arc<dyn Host>) {
    let _ = HOST.set(host);
}

/// Who answers for the workspace.
pub fn host() -> &'static dyn Host {
    HOST.get_or_init(|| Arc::new(Alone { yours: false }))
        .as_ref()
}

/// The diff answering for the workspace itself, with none running.
pub struct Alone {
    /// Whether what you set in Switchboard's settings file is read over
    /// the defaults.
    yours: bool,
}

impl Alone {
    /// As `switchboard-diff` runs: your settings over the defaults.
    pub fn new() -> Alone {
        Alone { yours: true }
    }
}

impl Default for Alone {
    fn default() -> Alone {
        Alone::new()
    }
}

/// The diff's own folders, alone. In this crate's unit tests, whose home
/// is yours, a folder of the system's temporary directory.
fn folders_alone() -> Folders {
    if cfg!(test) {
        let tmp = std::env::temp_dir().join("switchboard-diff-unit-tests");
        return Folders {
            state: tmp.join("state"),
            cache: tmp.join("cache"),
        };
    }
    Folders {
        state: home_join(".local/state/switchboard-diff"),
        cache: home_join(".cache/switchboard-diff"),
    }
}

/// Switchboard's settings file, when you have one: what you changed there.
fn saved() -> serde_json::Map<String, Value> {
    let file = match std::env::var_os("SWITCHBOARD_SETTINGS") {
        Some(f) if !f.is_empty() => PathBuf::from(f),
        _ => home_join(".config/switchboard/settings.json"),
    };
    std::fs::read_to_string(file)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

/// One setting alone: what you saved (read only with `yours`), when it is
/// of the default's kind (on/off for an on/off option, a word for a
/// choice), else the default; `None` for one neither saved nor listed here.
fn setting_alone(key: &str, yours: bool) -> Option<Value> {
    static SAVED: OnceLock<serde_json::Map<String, Value>> = OnceLock::new();
    let default = crate::manifest().default_of(key).or_else(|| {
        switchboard_ui::settings::DEFAULTS
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, d)| d.value())
    });
    let saved = yours.then(|| SAVED.get_or_init(saved).get(key)).flatten();
    match (saved, default) {
        (Some(v), Some(d)) if v.is_boolean() == d.is_boolean() => Some(v.clone()),
        (Some(v), None) => Some(v.clone()),
        (_, default) => default,
    }
}

impl Host for Alone {
    fn ask(&self, action: Action) -> Answer {
        match action {
            // Alone, GitHub is never asked, so a base kept as a branch's
            // with no PR is not asked for again in case one has come.
            Action::Setting { key } if key == "github" => Answer::Value {
                value: Value::Bool(false),
            },
            Action::Setting { key } => match setting_alone(&key, self.yours) {
                Some(value) => Answer::Value { value },
                None => Answer::None,
            },
            Action::Folders => Answer::Folders {
                folders: folders_alone(),
            },
            Action::InPane => Answer::Flag { on: false },
            // No list of whose code a directory holds: yours.
            Action::Untrusted { .. } => Answer::Flag { on: false },
            // The terminal's own clipboard (OSC 52) is the diff's to write.
            Action::Copy { .. } | Action::Request { .. } | Action::Fetch { .. } => Answer::Done,
            Action::Command { .. }
            | Action::Pr { .. }
            | Action::ReviewedPr { .. }
            | Action::HeadlessClaude
            | Action::Plan { .. } => Answer::None,
        }
    }
}

/// Gives the drawing crate the diff's settings, for the diff alone: the
/// workspace says its own before it runs the diff.
pub fn share_with_ui() {
    switchboard_ui::settings::install(switchboard_ui::settings::Source {
        cached: |key| host().setting(key),
        fresh: |keys| keys.iter().map(|key| host().setting(key)).collect(),
    });
}

/// Where the diff's files are, as its host says.
pub mod paths {
    use std::path::PathBuf;

    pub use switchboard_plugin_api::sdk::{home, home_join, tilde};

    fn folders() -> switchboard_plugin_api::Folders {
        super::host().folders().unwrap_or_else(super::folders_alone)
    }

    /// What must last: which commit's story was told.
    pub fn state_dir() -> PathBuf {
        folders().state
    }

    /// What can be made again: the stories, the orders, the bases.
    pub fn cache_dir() -> PathBuf {
        folders().cache
    }

    /// `~/.claude`, where Claude Code keeps its sessions.
    pub fn claude_dir() -> PathBuf {
        home_join(".claude")
    }
}

/// The settings, as the diff's host has them.
pub mod settings {
    use serde_json::Value;

    /// Whether an on/off option is on. One that isn't listed counts as on.
    pub fn is_on(key: &str) -> bool {
        super::host().is_on(key)
    }

    /// One option's value.
    pub fn value(key: &str) -> Option<Value> {
        super::host().setting(key)
    }
}

/// The request audit, which the workspace keeps.
pub mod requests {
    /// Counts a request of a service outside the computer.
    pub fn count(service: &str, protocol: &str, source: &str) {
        super::host().count_request(service, protocol, source);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alone_the_diff_answers_with_its_defaults_and_asks_nothing_of_a_workspace() {
        let alone = Alone { yours: false };
        assert_eq!(alone.setting("diff_story"), Some(Value::Bool(true)));
        assert_eq!(alone.setting("diff_story_plan"), Some(Value::Bool(false)));
        assert_eq!(alone.setting("theme"), Some(Value::String("dark".into())));
        assert!(alone.is_on("not listed"));
        assert!(!alone.in_pane());
        assert!(!alone.untrusted(std::path::Path::new("/tmp")));
        assert_eq!(alone.pr(std::path::Path::new("/tmp")), None);
        assert_eq!(alone.reviewed_pr(std::path::Path::new("/tmp")), None);
        assert!(!alone.is_on("github"), "alone, GitHub is never asked");
        assert_eq!(alone.command(&["say", "x"]), None);
        assert_eq!(alone.plan(None), "");
        let folders = alone.folders().unwrap();
        assert_ne!(folders.state, folders.cache);
    }
}
