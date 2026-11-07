//! The workspace's settings: every option is listed, with its category,
//! description and default, in `assets/settings.json` (compiled in), and what
//! you have changed is kept in `~/.config/switchboard/settings.json`, only the
//! keys you set, so an option you never touched follows its default, and
//! which you may edit by hand. Mission control's Settings view changes them
//! here, and the server reads them again whenever the file changes.
//!
//! An option is on/off (a boolean default) or one of its `choices`. A value in
//! the file that is neither is ignored, and the default used. A setting
//! renamed since it was saved (`RENAMED`) is read once as its new key and
//! dropped from the file when the server starts (`migrate`), as the sidebar
//! daemon's `migrateSettings` did.

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::paths;
use crate::util;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Choice {
    pub value: Value,
    pub label: String,
}

/// One option, with its value now (`value`), as `readSettings` gives it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Setting {
    pub key: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choices: Option<Vec<Choice>>,
    pub default: Value,
    /// The option's value now; the default until read.
    #[serde(default)]
    pub value: Value,
}

impl Setting {
    /// Whether `value` is one this option can take.
    pub fn valid(&self, value: &Value) -> bool {
        match &self.choices {
            Some(choices) => choices.iter().any(|c| &c.value == value),
            None => value.is_boolean(),
        }
    }

    /// The value after this one: on/off flips, a choice goes to the next.
    pub fn next_value(&self) -> Value {
        match &self.choices {
            None => Value::Bool(!util::truthy(&self.value)),
            Some(choices) => {
                let at = choices
                    .iter()
                    .position(|c| c.value == self.value)
                    .map(|i| i as isize)
                    .unwrap_or(-1);
                choices[((at + 1) as usize) % choices.len()].value.clone()
            }
        }
    }

    /// The value in words: "on", "off", or its choice's label.
    pub fn value_words(&self) -> String {
        match &self.choices {
            Some(choices) => choices
                .iter()
                .find(|c| c.value == self.value)
                .map(|c| c.label.clone())
                .unwrap_or_else(|| util::js_string(&self.value)),
            None => if util::truthy(&self.value) {
                "on"
            } else {
                "off"
            }
            .into(),
        }
    }

    /// Whether an on/off option is on; a choice counts as on.
    pub fn is_on(&self) -> bool {
        self.value != Value::Bool(false)
    }
}

/// The list of options, as compiled in.
pub const SCHEMA: &str = include_str!("../assets/settings.json");

/// `SWITCHBOARD_SETTINGS`, else `~/.config/switchboard/settings.json`. In
/// the library's own unit tests, whose home is yours, a file that isn't
/// there: every option reads its default, never what you have set.
pub fn settings_file() -> PathBuf {
    match std::env::var_os("SWITCHBOARD_SETTINGS") {
        Some(f) if !f.is_empty() => PathBuf::from(f),
        _ if cfg!(test) => PathBuf::from("/nonexistent/switchboard-unit-tests/settings.json"),
        _ => paths::config_dir().join("settings.json"),
    }
}

/// The options as listed, each with its default as its value.
pub fn schema() -> Vec<Setting> {
    let mut options: Vec<Setting> =
        serde_json::from_str(SCHEMA).expect("assets/settings.json is a list of options");
    for option in &mut options {
        option.value = option.default.clone();
    }
    options
}

/// What you have changed, or nothing when the file is missing or isn't an object.
fn saved() -> Map<String, Value> {
    util::read_json_object(settings_file())
}

/// Every option with its value now.
pub fn read_settings() -> Vec<Setting> {
    let mine = saved();
    schema()
        .into_iter()
        .map(|mut option| {
            if let Some(v) = mine.get(&option.key).filter(|v| option.valid(v)) {
                option.value = v.clone();
            }
            option
        })
        .collect()
}

/// Whether your file sets an option, rather than leaving it to its default:
/// a choice made, though one that matches the default is kept only when
/// written by hand, since `write_setting` keeps the file to what differs.
pub fn is_set(key: &str) -> bool {
    saved().contains_key(key)
}

/// One option's value now, or `None` for an option that isn't listed.
pub fn setting(key: &str) -> Option<Value> {
    read_settings()
        .into_iter()
        .find(|o| o.key == key)
        .map(|o| o.value)
}

// The settings as last read, for code that asks many times a second (the
// sidebar daemon draws several times a second): read again only by
// `reload_settings`, which the daemon calls when the file changes.
static CACHE: Mutex<Option<Vec<Setting>>> = Mutex::new(None);

/// The settings as last read (read now the first time).
pub fn current_settings() -> Vec<Setting> {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache.get_or_insert_with(read_settings).clone()
}

/// Reads the settings again into the cache, and gives them.
pub fn reload_settings() -> Vec<Setting> {
    let fresh = read_settings();
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some(fresh.clone());
    fresh
}

/// Whether an on/off option is on, from the cache. An option that isn't
/// listed counts as on, as `isOn` did.
pub fn is_on(key: &str) -> bool {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let settings = cache.get_or_insert_with(read_settings);
    settings
        .iter()
        .find(|o| o.key == key)
        .is_none_or(|o| o.value != Value::Bool(false))
}

/// One option's value from the cache.
pub fn cached(key: &str) -> Option<Value> {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let settings = cache.get_or_insert_with(read_settings);
    settings
        .iter()
        .find(|o| o.key == key)
        .map(|o| o.value.clone())
}

/// Sets one option. Keeps the file to what differs from the defaults, written
/// to a temporary file and renamed into place, so a reader never sees half of
/// it. A file that is there but can't be read as a JSON object is never
/// written over, since starting from nothing would lose every other setting in
/// it; the version before each write is kept beside it as `settings.json.bak`.
pub fn write_setting(key: &str, value: Value) -> Result<()> {
    let Some(option) = schema().into_iter().find(|o| o.key == key) else {
        bail!("not a value for {key}: {}", util::json_compact(&value));
    };
    if !option.valid(&value) {
        bail!("not a value for {key}: {}", util::json_compact(&value));
    }
    let file = settings_file();
    let mut mine = Map::new();
    if file.exists() {
        match fs::read_to_string(&file)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        {
            Some(Value::Object(map)) => mine = map,
            _ => bail!(
                "{} can't be read as JSON; fix it by hand, then change the setting again",
                paths::tilde(&file)
            ),
        }
        let mut bak = file.clone().into_os_string();
        bak.push(".bak");
        fs::copy(&file, PathBuf::from(bak))?;
    }
    if value == option.default {
        mine.shift_remove(key);
    } else {
        mine.insert(key.to_string(), value);
    }
    util::write_atomic(
        &file,
        format!("{}\n", util::json_pretty(&Value::Object(mine))),
    )?;
    Ok(())
}

/// Settings renamed since they were saved, old key first: the Merge queues
/// view became the Landing view.
pub const RENAMED: &[(&str, &str)] = &[("view_merge_queues", "view_landing")];

/// Your file with what an earlier version saved read as this one names it:
/// a renamed key's off kept under its new name (unless that is set already),
/// the old key dropped. `None` when there is nothing to change.
pub fn migrated(mine: &Map<String, Value>) -> Option<Map<String, Value>> {
    if !RENAMED.iter().any(|(old, _)| mine.contains_key(*old)) {
        return None;
    }
    let mut out = mine.clone();
    for (old, new) in RENAMED {
        if let Some(value) = out.shift_remove(*old)
            && value == Value::Bool(false)
            && !out.contains_key(*new)
        {
            out.insert(new.to_string(), value);
        }
    }
    Some(out)
}

/// Writes your file as `migrated` reads it, when it changes anything, the
/// version before kept as `settings.json.bak`; a file that can't be read
/// as a JSON object is left alone.
pub fn migrate() -> Result<()> {
    let file = settings_file();
    let Some(Value::Object(mine)) = fs::read_to_string(&file)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
    else {
        return Ok(());
    };
    let Some(out) = migrated(&mine) else {
        return Ok(());
    };
    let mut bak = file.clone().into_os_string();
    bak.push(".bak");
    fs::copy(&file, PathBuf::from(bak))?;
    util::write_atomic(
        &file,
        format!("{}\n", util::json_pretty(&Value::Object(out))),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_setting_saved_under_its_old_name_is_read_as_its_new_one_once() {
        let mine = |v: Value| v.as_object().unwrap().clone();
        assert_eq!(
            migrated(&mine(json!({ "view_merge_queues": false, "tips": false }))),
            Some(mine(json!({ "tips": false, "view_landing": false }))),
            "Merge queues off is the Landing view off"
        );
        assert_eq!(
            migrated(&mine(json!({ "view_merge_queues": true }))),
            Some(Map::new()),
            "on, the default, it is only dropped"
        );
        assert_eq!(migrated(&mine(json!({ "tips": false }))), None);
    }

    fn option(choices: Option<Vec<Choice>>, value: Value) -> Setting {
        Setting {
            key: "k".into(),
            category: String::new(),
            label: String::new(),
            description: String::new(),
            choices,
            default: value.clone(),
            value,
        }
    }

    #[test]
    fn on_off_flips_and_a_choice_goes_round() {
        assert_eq!(option(None, json!(true)).next_value(), json!(false));
        let choices = vec![
            Choice {
                value: json!("a"),
                label: "A".into(),
            },
            Choice {
                value: json!("b"),
                label: "B".into(),
            },
        ];
        assert_eq!(
            option(Some(choices.clone()), json!("b")).next_value(),
            json!("a")
        );
        assert_eq!(option(Some(choices.clone()), json!("a")).value_words(), "A");
        assert_eq!(option(None, json!(false)).value_words(), "off");
    }

    #[test]
    fn the_real_schema_reads() {
        let options = schema();
        assert!(options.len() > 10);
        assert!(
            options.iter().all(|o| o.valid(&o.default)),
            "every default is a value its option takes"
        );
    }
}
