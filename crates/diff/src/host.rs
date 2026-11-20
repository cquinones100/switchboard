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
//! as yours. It knows nothing of Switchboard: its settings are its own
//! defaults (the manifest's), over which it reads a file of its own,
//! `~/.config/switchboard-diff/settings.json`, never Switchboard's; its
//! state and cache are `~/.local/state/switchboard-diff` and
//! `~/.cache/switchboard-diff`.
//!
//! Alone, it also changes its own options (`write_setting`, from its
//! settings view), in that same file: only the
//! keys that differ from their defaults, the version before kept as
//! `settings.json.bak`, never over a file that doesn't read as a JSON
//! object. In the workspace they are Settings' to change.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{Map, Value};
use switchboard_plugin_api::sdk::{self, home_join};
use switchboard_plugin_api::{Action, Answer, Folders, Host};

static HOST: OnceLock<Arc<dyn Host>> = OnceLock::new();

/// Set once the diff runs as `switchboard-diff`, answering for itself.
static ALONE: AtomicBool = AtomicBool::new(false);

/// Whether the diff runs alone, as `switchboard-diff`, with no workspace:
/// its settings view, its Settings button and its look for a newer release
/// are there only then, the workspace having its own.
pub fn is_alone() -> bool {
    ALONE.load(Ordering::Relaxed)
}

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
        ALONE.store(true, Ordering::Relaxed);
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

/// The diff alone's own settings file, `~/.config/switchboard-diff/settings.json`
/// (`SWITCHBOARD_DIFF_SETTINGS` names another). In this crate's unit
/// tests, a file of the system's temporary directory.
pub fn settings_file() -> PathBuf {
    if cfg!(test) {
        return std::env::temp_dir().join("switchboard-diff-unit-tests/settings.json");
    }
    match std::env::var_os("SWITCHBOARD_DIFF_SETTINGS") {
        Some(f) if !f.is_empty() => PathBuf::from(f),
        _ => home_join(".config/switchboard-diff/settings.json"),
    }
}

/// The copy kept of the settings file before a change: its whole name and
/// `.bak`, as Switchboard names it.
fn bak_file(file: &std::path::Path) -> PathBuf {
    let mut name = file.as_os_str().to_os_string();
    name.push(".bak");
    PathBuf::from(name)
}

/// What you changed in the settings file, when you have one.
fn saved() -> Map<String, Value> {
    std::fs::read_to_string(settings_file())
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

/// The settings file as last read, read again after the diff writes it.
static SAVED: Mutex<Option<Map<String, Value>>> = Mutex::new(None);

fn saved_value(key: &str) -> Option<Value> {
    let mut kept = SAVED.lock().unwrap_or_else(|e| e.into_inner());
    kept.get_or_insert_with(saved).get(key).cloned()
}

/// Has the settings file read again at the next setting asked for, for the
/// diff alone as its settings view opens: what was changed since the diff
/// started shows there.
pub fn reread_settings() {
    *SAVED.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// What a setting of the diff's own is while you haven't set it.
fn default_of(key: &str) -> Option<Value> {
    crate::manifest().default_of(key)
}

/// Sets one of the diff's own options in the settings file, for the diff
/// alone: the key left out when the value is its default, the file's other
/// keys kept, the version before copied to `settings.json.bak`. Refused,
/// with why, for a key that isn't the diff's, a value of another kind than
/// its default, or a file that doesn't read as a JSON object.
pub fn write_setting(key: &str, value: Value) -> Result<(), String> {
    write_setting_at(&settings_file(), key, value)
}

/// `write_setting` in the file at `file`, the one the settings view
/// named as it opened.
pub fn write_setting_at(file: &std::path::Path, key: &str, value: Value) -> Result<(), String> {
    let Some(default) = default_of(key) else {
        return Err(format!("{key} isn't one of the diff's settings"));
    };
    if value.is_boolean() != default.is_boolean() || value.is_string() != default.is_string() {
        return Err(format!("not a value for {key}: {value}"));
    }
    // Written at its path as Switchboard writes it (a link there is
    // replaced by the file, as Settings does), the copy before beside it.
    let named = file.to_path_buf();
    let file = named.clone();
    let mut mine = Map::new();
    if file.exists() {
        match std::fs::read_to_string(&file)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        {
            Some(Value::Object(map)) => mine = map,
            _ => {
                return Err(format!(
                    "{} can't be read as JSON; fix it by hand, then change the setting again",
                    sdk::tilde(&file)
                ));
            }
        }
        std::fs::copy(&file, bak_file(&named))
            .map_err(|e| format!("couldn't keep {} as its .bak: {e}", sdk::tilde(&file)))?;
    } else if let Some(dir) = file.parent() {
        sdk::make_dirs(dir).map_err(|e| format!("couldn't make {}: {e}", sdk::tilde(dir)))?;
    }
    if value == default {
        mine.shift_remove(key);
    } else {
        mine.insert(key.to_string(), value);
    }
    let text = format!("{}\n", sdk::json_pretty(&Value::Object(mine.clone())));
    sdk::write_atomic(&file, text)
        .map_err(|e| format!("couldn't write {}: {e}", sdk::tilde(&file)))?;
    // Read again whole at the next setting asked for, what the local file
    // holds for the clone included (`saved`).
    reread_settings();
    Ok(())
}

/// One setting alone: what you saved (read only with `yours`), when it is
/// of the default's kind (on/off for an on/off option, a word for a
/// choice), else the default; `None` for one neither saved nor listed here.
fn setting_alone(key: &str, yours: bool) -> Option<Value> {
    let default = default_of(key).or_else(|| {
        switchboard_ui::settings::DEFAULTS
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, d)| d.value())
    });
    let saved = yours.then(|| saved_value(key)).flatten();
    match (saved, default) {
        (Some(v), Some(d)) if v.is_boolean() == d.is_boolean() => Some(v),
        (Some(v), None) => Some(v),
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
            // What only Notes asks.
            Action::Settings { .. }
            | Action::Paste
            | Action::PasteImage { .. }
            | Action::Preview { .. }
            | Action::LinkWords { .. }
            | Action::OpenLink { .. }
            | Action::Draws
            | Action::Picture { .. }
            | Action::PictureGone { .. }
            | Action::ItemClaim { .. }
            | Action::Queued
            | Action::Item { .. } => Answer::None,
            // The terminal's pointer is the diff's own to set (`App::point`).
            Action::Pointer { .. } => Answer::None,
            // No tab to mark.
            Action::Ready { .. } => Answer::None,
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

    #[test]
    fn a_setting_is_written_as_switchboard_writes_it_and_read_back() {
        let file = settings_file();
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "{\n  \"theme\": \"light\"\n}\n").unwrap();
        *SAVED.lock().unwrap() = None;
        let yours = Alone { yours: true };

        // Off is kept, beside what the file already held, the version
        // before as its .bak.
        write_setting("diff_story", Value::Bool(false)).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(
            text,
            "{\n  \"theme\": \"light\",\n  \"diff_story\": false\n}\n"
        );
        assert_eq!(yours.setting("diff_story"), Some(Value::Bool(false)));
        assert_eq!(
            std::fs::read_to_string(bak_file(&file)).unwrap(),
            "{\n  \"theme\": \"light\"\n}\n"
        );

        // Back to its default, the key is left out.
        write_setting("diff_story", Value::Bool(true)).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(text, "{\n  \"theme\": \"light\"\n}\n");
        assert_eq!(yours.setting("diff_story"), Some(Value::Bool(true)));

        // Never a key that isn't the diff's, a value of another kind, or
        // over a file that doesn't read.
        assert!(write_setting("theme", Value::String("dark".into())).is_err());
        assert!(write_setting("diff_story", Value::String("no".into())).is_err());
        std::fs::write(&file, "not json").unwrap();
        assert!(write_setting("diff_story", Value::Bool(false)).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "not json");
        let _ = std::fs::remove_file(&file);
        *SAVED.lock().unwrap() = None;
    }
}
