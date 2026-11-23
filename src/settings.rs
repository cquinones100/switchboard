//! The workspace's settings: every option is listed, with its category,
//! description and default, in `assets/settings.json` (compiled in), and what
//! you have changed is kept in `~/.config/switchboard/settings.json`, only the
//! keys you set, so an option you never touched follows its default, and
//! which you may edit by hand. Mission control's Settings view changes them
//! here, and the server reads them again whenever the file changes.
//!
//! An option is on/off (a boolean default) or one of its `choices`, or for
//! one with `typed` a number from its least, or a folder, typed in. A value in
//! the file that is neither is ignored, and the default used. A setting
//! renamed since it was saved (`RENAMED`) is read once as its new key and
//! dropped from the file when the server starts (`migrate`), as the sidebar
//! daemon's `migrateSettings` did; an on/off setting that became a choice
//! (`REVALUED`) has its saved on or off read as the choice that means the
//! same, and written over as that choice then.
//!
//! The settings that depend on the computer (`"scope": "computer"`,
//! `COMPUTER`: whether a service is used here, what runs on one computer
//! only, the hardware) are kept only in this computer's own file, the one
//! in `~/.config/switchboard` even while your files come from a repository
//! (src/files), and a value for one in the synced file is ignored. They are
//! always written, even at their default, since a service's setting left
//! unset is one not yet seen working here: off until it has
//! (`turn_on_where_it_works`), when it turns on, so a new computer asks
//! nothing of a service that isn't set up on it. The build that made them
//! the computer's own carries each computer's values over once
//! (`carry_over`).

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

/// A value an option with choices also takes when typed in: a whole number
/// (`lone_pane_width`'s columns) or a folder (`new_tab_home`). The kind,
/// the least number it takes, the label of the menu option that asks for
/// it, and the words after a number when it is shown ("columns").
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Typed {
    #[serde(default)]
    pub kind: TypedKind,
    #[serde(default)]
    pub least: u64,
    pub label: String,
    #[serde(default)]
    pub unit: String,
}

/// What a typed-in value is.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TypedKind {
    #[default]
    Number,
    /// A folder, written `~/...` or as an absolute path.
    Folder,
    /// Any words on one line (a note's title, `coordinator_note`).
    Text,
}

impl Typed {
    /// The number `value` holds, written as the choices are (a string of
    /// digits), when it is one this takes.
    pub fn number(&self, value: &Value) -> Option<u64> {
        if self.kind != TypedKind::Number {
            return None;
        }
        value.as_str()?.parse().ok().filter(|n| *n >= self.least)
    }

    /// The folder `value` names (`~/` read as the home), when this takes
    /// a folder and it is written `~/...` or as an absolute path; whether
    /// it is there is the caller's to ask.
    pub fn folder(&self, value: &Value) -> Option<PathBuf> {
        if self.kind != TypedKind::Folder {
            return None;
        }
        let text = value.as_str()?.trim();
        match text.strip_prefix("~/") {
            Some(rest) => Some(paths::home().join(rest)),
            None if text == "~" => Some(paths::home()),
            None => Some(PathBuf::from(text)).filter(|p| p.is_absolute()),
        }
    }

    /// The words `value` holds, trimmed, when this takes words and they
    /// are on one line and not empty.
    pub fn text(&self, value: &Value) -> Option<String> {
        if self.kind != TypedKind::Text {
            return None;
        }
        let text = value.as_str()?.trim();
        (!text.is_empty() && !text.contains(['\n', '\r'])).then(|| text.to_string())
    }

    /// Whether `value` is one this takes typed in.
    pub fn takes(&self, value: &Value) -> bool {
        self.number(value).is_some() || self.folder(value).is_some() || self.text(value).is_some()
    }

    /// `value` as shown: the number and its unit, or the folder as written.
    pub fn words(&self, value: &Value) -> Option<String> {
        match self.kind {
            TypedKind::Number => Some(format!("{} {}", self.number(value)?, self.unit)),
            TypedKind::Folder => {
                self.folder(value)?;
                value.as_str().map(|t| t.trim().to_string())
            }
            TypedKind::Text => self.text(value),
        }
    }
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
    /// A number it takes typed in besides its choices, for the few that do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typed: Option<Typed>,
    pub default: Value,
    /// Whether it follows you to every computer or is this computer's own.
    #[serde(default, skip_serializing_if = "Scope::is_synced")]
    pub scope: Scope,
    /// A computer setting's default on a computer where it isn't set, which
    /// `schema` makes its `default`; `default` itself stays what a build
    /// from before computer settings reads, from a clone's schema too, so a
    /// workspace still running one keeps what it used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub computer_default: Option<Value>,
    /// The option's value now; the default until read.
    #[serde(default)]
    pub value: Value,
}

/// Where an option's value is kept: in `settings.json`, which follows you
/// to every computer while your files come from a repository, or in this
/// computer's own file alone (`"scope": "computer"`), for what depends on
/// the computer: a service used here, what runs on one computer only, the
/// hardware.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    #[default]
    Synced,
    Computer,
}

impl Scope {
    pub fn is_synced(&self) -> bool {
        *self == Scope::Synced
    }
}

impl Setting {
    /// Whether `value` is one this option can take.
    pub fn valid(&self, value: &Value) -> bool {
        match &self.choices {
            Some(choices) => {
                choices.iter().any(|c| &c.value == value)
                    || self.typed.as_ref().is_some_and(|t| t.takes(value))
            }
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
                .or_else(|| self.typed.as_ref()?.words(&self.value))
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

    /// Whether its value differs from its default.
    pub fn changed(&self) -> bool {
        self.value != self.default
    }

    /// Its default in words, as `value_words` gives its value.
    pub fn default_words(&self) -> String {
        Setting {
            value: self.default.clone(),
            ..self.clone()
        }
        .value_words()
    }
}

/// A category named `group: name` (`Mission control: Slack`) as its group
/// and its name under it: the Settings view draws the group as a heading
/// and the category under it. None for a category with no group.
pub fn category_group(category: &str) -> Option<(&str, &str)> {
    category.split_once(": ")
}

/// The group a category belongs to among `categories`: its own, or, for a
/// category some other names as its group (`Mission control` beside
/// `Mission control: Slack`), itself, since it holds what belongs to the
/// group as a whole.
pub fn group_of<'a>(category: &'a str, categories: &[&str]) -> Option<&'a str> {
    if let Some((group, _)) = category_group(category) {
        return Some(group);
    }
    categories
        .iter()
        .any(|c| category_group(c).is_some_and(|(group, _)| group == category))
        .then_some(category)
}

/// The options' categories in the order the Settings view lists them: the
/// ones with no group as the options first name them, then each group's
/// together, the category holding what belongs to the group as a whole
/// first. So an option added anywhere in the file lands under its group.
pub fn categories(all: &[Setting]) -> Vec<&str> {
    let mut named: Vec<&str> = Vec::new();
    for o in all {
        if !named.contains(&o.category.as_str()) {
            named.push(&o.category);
        }
    }
    let mut groups: Vec<&str> = Vec::new();
    for c in &named {
        if let Some(group) = group_of(c, &named)
            && !groups.contains(&group)
        {
            groups.push(group);
        }
    }
    let mut out: Vec<&str> = named
        .iter()
        .copied()
        .filter(|c| group_of(c, &named).is_none())
        .collect();
    for group in groups {
        out.extend(named.iter().copied().filter(|c| *c == group));
        out.extend(
            named
                .iter()
                .copied()
                .filter(|c| *c != group && group_of(c, &named) == Some(group)),
        );
    }
    out
}

/// The list of options, as compiled in; `schema` reads the clone's first
/// (src/assets.rs).
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

/// The options that are this computer's own (`"scope": "computer"`), read
/// from and written to `~/.config/switchboard/settings.json` even while
/// your files are kept in a git repository (src/files), never from the
/// synced `settings.json`: whether a service is used here, what runs on one
/// computer only, the hardware, and whether your files come from a
/// repository, since that says where the rest are read from. A test keeps
/// this the same as the schema's.
pub const COMPUTER: &[&str] = &[
    "github",
    "linear",
    "view_slack",
    "view_calendar",
    "meeting_notes",
    "chrome_profiles",
    "pr_merge_queue",
    "autopilot",
    "view_relay",
    "phone",
    "slot_queue",
    "slots_build",
    "slots_tests",
    "slots_js",
    "keep_awake",
    "worktree_clone_build",
    crate::files::SETTING,
];

/// Whether an option is this computer's own.
pub fn is_computer(key: &str) -> bool {
    COMPUTER.contains(&key)
}

/// The settings that say whether a service is used here, off on a computer
/// until the service has worked there, with the connector whose answer
/// tells it has (`connectors.json`'s service), where Switchboard calls one:
/// Slack's from a search. The rest are told by what is found on the
/// computer (`found_here`: gh's sign-in, a Linear key, Chrome's profiles,
/// a meeting's notes, your own Slack sign-in) or read there (the
/// calendar's feed).
pub const SERVICES: &[(&str, Option<&str>)] = &[
    ("github", None),
    ("linear", None),
    ("view_slack", Some("Slack")),
    ("view_calendar", Some("Google Calendar")),
    ("meeting_notes", Some("Fathom")),
    ("chrome_profiles", None),
];

/// A service setting's default in words, on a computer where it isn't set:
/// off until what turns it on, as board 183 draws it.
pub fn default_here(key: &str) -> Option<&'static str> {
    Some(match key {
        "github" => "off until gh signs in here",
        "linear" => "off until a Linear key is stored here",
        "view_slack" => "off until Slack answers here",
        "view_calendar" => "off until your calendar is read here",
        "meeting_notes" => "off until Fathom's notes are written here",
        "chrome_profiles" => "off until Chrome has a profile here",
        _ => return None,
    })
}

/// The setting that says whether a connector's service is used here.
pub fn setting_of_connector(service: &str) -> Option<&'static str> {
    SERVICES
        .iter()
        .find(|(_, s)| *s == Some(service))
        .map(|(key, _)| *key)
}

/// Whether a connector's service may be asked about on this computer: one
/// a setting says is used here only while that setting is on, so a
/// computer where it has never worked asks nothing of it, not even whether
/// it is connected.
pub fn connector_used_here(service: &str) -> bool {
    setting_of_connector(service).is_none_or(is_on)
}

/// What the computer settings that changed their default with them being
/// the computer's own (off until a service works, or turned on where you
/// want it) were before: what a computer that already ran Switchboard
/// keeps, as `carry_over` writes it.
const DEFAULT_BEFORE: &[(&str, bool)] = &[
    ("github", true),
    ("linear", true),
    ("view_slack", true),
    ("view_calendar", true),
    ("meeting_notes", true),
    ("chrome_profiles", true),
    ("autopilot", true),
    ("view_relay", true),
];

/// This computer's own settings file: `settings_file` unless your files
/// come from a repository, then the one in `~/.config/switchboard`.
pub fn local_settings_file() -> PathBuf {
    match std::env::var_os("SWITCHBOARD_SETTINGS") {
        Some(f) if !f.is_empty() => PathBuf::from(f),
        _ if cfg!(test) => PathBuf::from("/nonexistent/switchboard-unit-tests/settings.json"),
        _ => paths::local_config_dir().join("settings.json"),
    }
}

/// `settings.json.bak`, the version before the last write: beside this
/// computer's own settings file, whichever file the write was to.
fn bak_file() -> PathBuf {
    let mut bak = local_settings_file().into_os_string();
    bak.push(".bak");
    let bak = PathBuf::from(bak);
    if let Some(dir) = bak.parent() {
        let _ = util::make_dirs(dir);
    }
    bak
}

/// The file an option is kept in.
fn file_of(key: &str) -> PathBuf {
    file_among(key, settings_file(), local_settings_file())
}

/// The file an option is kept in, of the synced file and this computer's.
fn file_among(key: &str, synced: PathBuf, own: PathBuf) -> PathBuf {
    if is_computer(key) { own } else { synced }
}

/// The options as listed, each with its default as its value.
pub fn schema() -> Vec<Setting> {
    let mut options: Vec<Setting> =
        serde_json::from_value(crate::assets::json("settings.json", SCHEMA))
            .or_else(|_| serde_json::from_str(SCHEMA))
            .expect("assets/settings.json is a list of options");
    for option in &mut options {
        if let Some(default) = option.computer_default.take() {
            option.default = default;
        }
        option.value = option.default.clone();
    }
    options
}

/// Gives a setting its default back: a computer setting unset in this
/// computer's file, so a service's turns on again once it works here, any
/// other written as its default.
pub fn reset_setting(key: &str) -> Result<()> {
    if !is_computer(key) {
        let Some(option) = schema().into_iter().find(|o| o.key == key) else {
            bail!("no setting named {key}");
        };
        return write_setting(key, option.default);
    }
    let file = file_of(key);
    let mut mine = read_to_change(&file)?;
    if mine.shift_remove(key).is_some() {
        write_whole(&file, mine)?;
    }
    Ok(())
}

/// What you have changed, or nothing when the file is missing or isn't an object.
fn saved() -> Map<String, Value> {
    // The library's own unit tests read as a computer that ran Switchboard
    // before, its services in use, as the integration tests' servers do.
    if cfg!(test) {
        return DEFAULT_BEFORE
            .iter()
            .map(|(k, v)| (k.to_string(), Value::Bool(*v)))
            .collect();
    }
    let synced = util::read_json_object(settings_file());
    let local = local_settings_file();
    if local == settings_file() {
        return synced;
    }
    with_computer_s(synced, &util::read_json_object(local))
}

/// The synced settings with the computer's own over them: each computer
/// setting as this computer's file has it, and unset where that file
/// doesn't set it, whatever the synced file says.
fn with_computer_s(mut synced: Map<String, Value>, own: &Map<String, Value>) -> Map<String, Value> {
    for key in COMPUTER {
        match own.get(*key) {
            Some(value) => synced.insert(key.to_string(), value.clone()),
            None => synced.shift_remove(*key),
        };
    }
    synced
}

/// Settings that were on/off and became choices: each old value's choice.
pub const REVALUED: &[(&str, &str, &str)] = &[("autopilot_close", "seen", "off")];

/// A saved value as its setting reads it now: an on/off of one that became
/// a choice (`REVALUED`) is the choice that means the same.
pub fn revalued(key: &str, value: &Value) -> Value {
    match (REVALUED.iter().find(|(k, _, _)| *k == key), value) {
        (Some((_, on, _)), Value::Bool(true)) => Value::String(on.to_string()),
        (Some((_, _, off)), Value::Bool(false)) => Value::String(off.to_string()),
        _ => value.clone(),
    }
}

/// Every option with its value now.
pub fn read_settings() -> Vec<Setting> {
    let mine = saved();
    schema()
        .into_iter()
        .map(|mut option| {
            if let Some(v) = mine
                .get(&option.key)
                .map(|v| revalued(&option.key, v))
                .filter(|v| option.valid(v))
            {
                option.value = v;
            }
            option
        })
        .collect()
}

/// Whether your file sets an option, rather than leaving it to its default:
/// a choice made, though one that matches the default is kept only when
/// written by hand, since `write_setting` keeps the file to what differs
/// (a computer setting kept whatever its value).
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

/// The heading the settings of this computer's own are listed under first
/// in Settings, besides their own categories.
pub const THIS_COMPUTER: &str = "This computer";

/// The category of the settings that serve developing Switchboard, shown
/// only on a build made from its clone (`dev::build_is_from_clone`).
pub const DEVELOPING: &str = "Developing Switchboard";

/// The settings as Settings lists them: `current_settings`, without
/// `DEVELOPING`'s unless the running build was made from Switchboard's
/// clone, where alone they act on something. They keep their values and
/// `switchboard setting` still reads and writes them.
pub fn shown_settings() -> Vec<Setting> {
    shown_of(current_settings(), crate::dev::build_is_from_clone())
}

fn shown_of(mut all: Vec<Setting>, from_clone: bool) -> Vec<Setting> {
    if !from_clone {
        all.retain(|o| o.category != DEVELOPING);
    }
    all
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

/// The folder a folder option names, from the cache, while it is one
/// (`new_tab_home`): `None` while it is left at its default, or names
/// something that isn't a folder.
pub fn folder(key: &str) -> Option<PathBuf> {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let settings = cache.get_or_insert_with(read_settings);
    let option = settings.iter().find(|o| o.key == key)?;
    option
        .typed
        .as_ref()?
        .folder(&option.value)
        .filter(|d| d.is_dir())
}

/// Tells the drawing crate (`switchboard-ui`, which knows no settings file)
/// where the settings it follows are: these. Every program of the binary
/// says so as it starts (`cmd::run`); until then that crate answers the
/// defaults it lists, which a test here keeps the same as the schema's.
pub fn share_with_ui() {
    switchboard_ui::settings::install(switchboard_ui::settings::Source {
        cached,
        fresh: |keys| {
            let all = read_settings();
            keys.iter()
                .map(|key| all.iter().find(|o| o.key == *key).map(|o| o.value.clone()))
                .collect()
        },
    });
}

/// Sets one option, a computer setting in this computer's own file and any
/// other in `settings.json`. Keeps the file to what differs from the
/// defaults (a computer setting kept whatever its value), written
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
    let file = file_of(key);
    let mut mine = read_to_change(&file)?;
    // A computer setting is kept even at its default, since one left unset
    // is a service not yet seen working here (`turn_on_where_it_works`).
    if value == option.default && !is_computer(key) {
        mine.shift_remove(key);
    } else {
        mine.insert(key.to_string(), value);
    }
    write_whole(&file, mine)
}

/// A settings file as an object to change, after keeping the version
/// before as `settings.json.bak`: nothing when it is missing, an error
/// when it is there and can't be read as one, since starting from nothing
/// would lose every other setting in it.
fn read_to_change(file: &std::path::Path) -> Result<Map<String, Value>> {
    if !file.exists() {
        return Ok(Map::new());
    }
    match fs::read_to_string(file)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
    {
        Some(Value::Object(map)) => {
            // The copy stays on this computer, whatever file it is of.
            fs::copy(file, bak_file())?;
            Ok(map)
        }
        _ => bail!(
            "{} can't be read as JSON; fix it by hand, then change the setting again",
            paths::tilde(file)
        ),
    }
}

fn write_whole(file: &std::path::Path, mine: Map<String, Value>) -> Result<()> {
    util::write_atomic(
        file,
        format!("{}\n", util::json_pretty(&Value::Object(mine))),
    )?;
    Ok(())
}

/// Turns on a service's setting on this computer once the service has
/// worked here, unless this computer's file already says whether it is
/// used, which is then left as you set it.
pub fn turn_on_where_it_works(key: &str) {
    // Not before the carry-over, which may yet bring this computer's own
    // value for it from the synced file.
    if cfg!(test) || !SERVICES.iter().any(|(k, _)| *k == key) || !carried_marker().exists() {
        return;
    }
    let file = local_settings_file();
    if util::read_json_object(&file).contains_key(key) {
        return;
    }
    if let Err(e) = write_setting(key, Value::Bool(true)) {
        eprintln!("switchboard: could not turn {key} on: {e:#}");
    }
}

/// A connector that answered: its service's setting turned on here.
pub fn connector_worked(service: &str) {
    if let Some(key) = setting_of_connector(service) {
        turn_on_where_it_works(key);
    }
}

/// The services a sign-in on this computer shows are used here, read from
/// small files and the environment alone, so the server can look once a
/// minute: gh signed in, a Linear key in the environment or linear.json,
/// a Chrome with profiles. A Linear key in the Keychain is found as it is
/// stored (`switchboard platform secret-set`) or first read.
pub fn found_here() -> Vec<&'static str> {
    let env = |name: &str| std::env::var(name).is_ok_and(|v| !v.trim().is_empty());
    // Where gh keeps its sign-in, as gh looks for it.
    let var = |name: &str| {
        std::env::var_os(name)
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
    };
    let gh_dir = var("GH_CONFIG_DIR")
        .or_else(|| var("XDG_CONFIG_HOME").map(|d| d.join("gh")))
        .unwrap_or_else(|| paths::home_join(".config/gh"));
    let gh = env("GH_TOKEN")
        || env("GITHUB_TOKEN")
        || fs::read_to_string(gh_dir.join("hosts.yml")).is_ok_and(|t| t.contains("user:"));
    let linear = env("LINEAR_API_KEY")
        || util::read_json_object(crate::files::file("linear.json"))
            .get("linearApiKey")
            .and_then(Value::as_str)
            .is_some_and(|k| !k.trim().is_empty());
    let chrome = crate::platform::chrome_local_state().is_some_and(|f| f.is_file());
    // The built-in meeting notes' record, written once Fathom has given a
    // meeting here.
    let fathom = paths::state_dir().join("meetings.json").is_file();
    // Your own Slack sign-in, which the Slack view reads with.
    let slack = crate::day::slack_user::signed_in(&paths::state_dir()).is_some();
    [
        ("github", gh),
        ("linear", linear),
        ("chrome_profiles", chrome),
        ("meeting_notes", fathom),
        ("view_slack", slack),
    ]
    .into_iter()
    .filter(|(_, found)| *found)
    .map(|(key, _)| key)
    .collect()
}

/// Turns on each service `found_here` finds, where this computer's file
/// doesn't say otherwise.
pub fn turn_on_found() {
    for key in found_here() {
        turn_on_where_it_works(key);
    }
}

/// The record that this computer's settings were carried over, in the
/// state directory.
fn carried_marker() -> PathBuf {
    paths::state_dir().join("computer-settings-carried")
}

/// Carries what this computer had over into its own file, once, as the
/// build that made settings the computer's own first starts: on a computer
/// that already ran Switchboard (a saved layout), each computer setting its
/// own file doesn't set takes the synced file's value, else the default it
/// read before (`DEFAULT_BEFORE`), so nothing it used turns off; a new
/// computer takes nothing from another's. A value the computer's file sets
/// is kept, and the synced file is
/// left as it is, for a computer that hasn't run this build yet; run again
/// (its record in a new state directory), it only fills what is missing.
pub fn carry_over() -> Result<()> {
    let marker = carried_marker();
    if cfg!(test) || marker.exists() {
        return Ok(());
    }
    let state = paths::state_dir();
    let ran_before = ["layout.json", "layout.json.1"]
        .iter()
        .any(|f| state.join(f).exists());
    let file = local_settings_file();
    // A synced file there but unreadable is carried from once it is fixed,
    // not taken for one that sets nothing.
    let synced_file = settings_file();
    let synced = match fs::read_to_string(&synced_file) {
        Err(_) => Map::new(),
        Ok(text) if text.trim().is_empty() => Map::new(),
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(map)) => map,
            _ => bail!(
                "{} can't be read as JSON; fix it by hand, and this computer's settings are carried from it then",
                paths::tilde(&synced_file)
            ),
        },
    };
    // Read without the backup first, which only a write takes.
    let mut wanted = util::read_json_object(&file);
    if carry_into(&mut wanted, &synced, ran_before) {
        let mut mine = read_to_change(&file)?;
        for key in COMPUTER {
            if let (false, Some(value)) = (mine.contains_key(*key), wanted.get(*key)) {
                mine.insert(key.to_string(), value.clone());
            }
        }
        if let Some(dir) = file.parent() {
            util::make_dirs(dir)?;
        }
        write_whole(&file, mine)?;
    }
    util::make_dirs(&state)?;
    util::write_atomic(&marker, format!("{}\n", util::now_ms()))?;
    Ok(())
}

/// This computer's file (`own`) with each computer setting it doesn't set
/// taken from the synced file, or the default read before, as `carry_over`
/// says; whether it changed.
fn carry_into(own: &mut Map<String, Value>, synced: &Map<String, Value>, ran_before: bool) -> bool {
    let schema = schema();
    let mut changed = false;
    for key in COMPUTER {
        if own.contains_key(*key) {
            continue;
        }
        let valid = |v: &&Value| schema.iter().any(|o| o.key == *key && o.valid(v));
        let before = DEFAULT_BEFORE
            .iter()
            .find(|(k, _)| k == key)
            .filter(|_| ran_before)
            .map(|(_, v)| Value::Bool(*v));
        let theirs = synced
            .get(*key)
            .filter(valid)
            .filter(|_| ran_before)
            .cloned();
        if let Some(value) = theirs.or(before) {
            own.insert(key.to_string(), value);
            changed = true;
        }
    }
    changed
}

/// Settings renamed since they were saved, old key first: the Merge queues
/// view became the Landing view, and the Closed view the Reopen view. A
/// setting that became a choice is in
/// `REVALUED`.
pub const RENAMED: &[(&str, &str)] = &[
    ("view_merge_queues", "view_landing"),
    ("view_closed", "view_reopen"),
];

/// Your file with what an earlier version saved read as this one names it:
/// a renamed key's off kept under its new name (unless that is set already),
/// the old key dropped. `None` when there is nothing to change.
pub fn migrated(mine: &Map<String, Value>) -> Option<Map<String, Value>> {
    let stale: Vec<&str> = REVALUED
        .iter()
        .map(|(k, _, _)| *k)
        .filter(|k| mine.get(*k).is_some_and(Value::is_boolean))
        .collect();
    if stale.is_empty() && !RENAMED.iter().any(|(old, _)| mine.contains_key(*old)) {
        return None;
    }
    let mut out = mine.clone();
    // An on/off saved for a setting that became a choice is written as its
    // choice, the default's dropped.
    for key in stale {
        let value = revalued(key, &out[key]);
        let default = schema()
            .into_iter()
            .find(|o| o.key == key)
            .map(|o| o.default);
        if default.as_ref() == Some(&value) {
            out.shift_remove(key);
        } else {
            out.insert(key.to_string(), value);
        }
    }
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
    fs::copy(&file, bak_file())?;
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
    fn developing_switchboard_is_shown_only_on_a_build_from_its_clone() {
        let developing = [
            "review_before_push",
            "assets_from_checkout",
            "build_before_restart",
            "build_behind_notice",
            "worktree_clone_build",
        ];
        let all = schema();
        for key in developing {
            let listed = all.iter().find(|o| o.key == key).unwrap();
            assert_eq!(listed.category, DEVELOPING, "{key}");
        }
        let has = |shown: &[Setting]| categories(shown).contains(&DEVELOPING);
        assert!(has(&shown_of(all.clone(), true)));
        let release = shown_of(all.clone(), false);
        assert!(!has(&release));
        assert_eq!(
            release.len() + developing.len(),
            all.len(),
            "only its options are left out"
        );
    }

    #[test]
    fn the_drawing_crate_s_defaults_are_the_schema_s() {
        let schema = schema();
        for (key, default) in switchboard_ui::settings::DEFAULTS {
            let listed = schema.iter().find(|o| o.key == *key);
            assert_eq!(
                listed.map(|o| o.default.clone()),
                Some(default.value()),
                "{key}"
            );
        }
    }

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

    #[test]
    fn an_on_or_off_saved_for_a_setting_that_became_a_choice_reads_as_its_choice() {
        let mine = |v: Value| v.as_object().unwrap().clone();
        assert_eq!(revalued("autopilot_close", &json!(true)), json!("seen"));
        assert_eq!(revalued("autopilot_close", &json!(false)), json!("off"));
        assert_eq!(revalued("autopilot_close", &json!("5m")), json!("5m"));
        assert_eq!(revalued("tips", &json!(false)), json!(false));
        assert_eq!(
            migrated(&mine(json!({ "autopilot_close": false }))),
            Some(mine(json!({ "autopilot_close": "off" }))),
            "off is written as never"
        );
        assert_eq!(
            migrated(&mine(json!({ "autopilot_close": true }))),
            Some(Map::new()),
            "on, the default's when seen, is dropped"
        );
    }

    fn option(choices: Option<Vec<Choice>>, value: Value) -> Setting {
        Setting {
            key: "k".into(),
            category: String::new(),
            label: String::new(),
            description: String::new(),
            choices,
            typed: None,
            default: value.clone(),
            scope: Scope::Synced,
            computer_default: None,
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

    #[test]
    fn each_service_says_what_turns_it_on() {
        for (key, _) in SERVICES {
            assert!(default_here(key).is_some(), "{key}");
        }
    }

    #[test]
    fn every_computer_setting_is_one_of_the_list() {
        let marked: Vec<String> = schema()
            .into_iter()
            .filter(|o| o.scope == Scope::Computer)
            .map(|o| o.key)
            .collect();
        let mut listed: Vec<&str> = COMPUTER.to_vec();
        listed.sort();
        let mut sorted: Vec<&str> = marked.iter().map(String::as_str).collect();
        sorted.sort();
        assert_eq!(sorted, listed);
        for (key, _) in SERVICES
            .iter()
            .chain(&[("autopilot", None), ("view_relay", None)])
        {
            let o = schema().into_iter().find(|o| o.key == *key).unwrap();
            assert_eq!(
                o.default,
                json!(false),
                "{key} starts off on a new computer"
            );
            let before = DEFAULT_BEFORE.iter().find(|(k, _)| k == key).unwrap().1;
            let raw: Vec<Setting> = serde_json::from_str(SCHEMA).unwrap();
            let listed = raw.iter().find(|o| o.key == *key).unwrap();
            assert_eq!(
                listed.default,
                json!(before),
                "{key}: an older build's default"
            );
        }
    }

    #[test]
    fn a_computer_setting_is_read_from_the_computer_s_file_alone() {
        let map = |v: Value| v.as_object().unwrap().clone();
        let synced = map(json!({ "pr_merge_queue": true, "view_slack": true, "tips": false }));
        let own = map(json!({ "view_slack": false }));
        let read = with_computer_s(synced, &own);
        assert_eq!(
            read.get("pr_merge_queue"),
            None,
            "the synced value is ignored"
        );
        assert_eq!(read["view_slack"], json!(false), "the computer's own wins");
        assert_eq!(read["tips"], json!(false), "the rest stay synced");
    }

    #[test]
    fn a_computer_setting_is_written_to_the_computer_s_file() {
        let (synced, own) = (PathBuf::from("synced.json"), PathBuf::from("own.json"));
        assert_eq!(file_among("tips", synced.clone(), own.clone()), synced);
        for key in COMPUTER {
            assert_eq!(file_among(key, synced.clone(), own.clone()), own, "{key}");
        }
    }

    #[test]
    fn the_carry_over_copies_what_the_computer_lacks_and_never_overwrites() {
        let map = |v: Value| v.as_object().unwrap().clone();
        let synced = map(json!({ "slots_build": "2", "pr_merge_queue": true, "tips": false }));
        // A new computer: nothing of another computer's.
        let mut own = map(json!({ "files_from_repo": true }));
        assert!(!carry_into(&mut own, &synced, false));
        assert_eq!(own, map(json!({ "files_from_repo": true })));
        // One that ran before: its own values kept, the defaults it read
        // then written.
        let mut own = map(json!({ "pr_merge_queue": false }));
        assert!(carry_into(&mut own, &synced, true));
        assert_eq!(own["pr_merge_queue"], json!(false), "its own is kept");
        assert_eq!(own["slots_build"], json!("2"));
        assert_eq!(own["view_slack"], json!(true), "on before, by default");
        assert_eq!(own["autopilot"], json!(true));
        assert_eq!(own.get("tips"), None, "only the computer's settings");
        assert_eq!(own.get("phone"), None, "a default that didn't change");
        // Run again, it changes nothing.
        let again = own.clone();
        assert!(!carry_into(&mut own, &synced, true));
        assert_eq!(own, again);
    }

    #[test]
    fn looking_often_is_off_at_first() {
        let fast = schema().into_iter().find(|o| o.key == "poll_fast").unwrap();
        assert!(
            !fast.default.as_bool().unwrap(),
            "about eight times the requests to GitHub"
        );
    }
}
