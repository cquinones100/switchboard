//! The settings that live in your own files in `~/.config/switchboard`
//! rather than in settings.json, each with a row in mission control's
//! Settings view (`own_items`), under a category named after its file: a
//! word, a number, a list or a map is typed in there, and anything bigger
//! (the on-call channels, a file of your own) opens in an editor. A new file
//! there, or a new setting in one, gets an entry in `OWN_FILES`. Ported from
//! lib/sidebar/own-config.js.
//!
//! The rows for the dotfiles' own overrides (tmux.conf, nvim.lua, Ghostty,
//! lazygit.yml) are gone with those dotfiles; keys.json, which changes the
//! bindings, takes their place.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use serde_json::{Map, Value, json};

use crate::items::{Item, State};
use crate::{paths, platform, util};

/// How a row's value is typed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    /// A whole number, at least the row's `min`.
    Number,
    /// Comma-separated.
    List,
    /// `key=value`, comma-separated.
    Map,
    /// One of Chrome's profiles, picked from a menu.
    Profile,
    /// The whole file, opened in an editor.
    File,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Text => "text",
            Kind::Number => "number",
            Kind::List => "list",
            Kind::Map => "map",
            Kind::Profile => "profile",
            Kind::File => "file",
        }
    }
}

/// One setting in a file.
#[derive(Clone, Copy, Debug)]
pub struct Row {
    pub key: Option<&'static str>,
    pub label: &'static str,
    pub kind: Kind,
    pub min: Option<i64>,
    pub detail: &'static str,
}

/// One file of yours and its settings.
#[derive(Clone, Copy, Debug)]
pub struct OwnFile {
    pub file: &'static str,
    pub category: &'static str,
    pub rows: &'static [Row],
}

const fn row(key: &'static str, label: &'static str, kind: Kind, detail: &'static str) -> Row {
    Row {
        key: Some(key),
        label,
        kind,
        min: None,
        detail,
    }
}

const fn number(key: &'static str, label: &'static str, min: i64, detail: &'static str) -> Row {
    Row {
        key: Some(key),
        label,
        kind: Kind::Number,
        min: Some(min),
        detail,
    }
}

const fn whole(label: &'static str, detail: &'static str) -> Row {
    Row {
        key: None,
        label,
        kind: Kind::File,
        min: None,
        detail,
    }
}

pub const OWN_FILES: &[OwnFile] = &[
    OwnFile {
        file: "claude.json",
        category: "Claude Code (claude.json)",
        rows: &[row(
            "file",
            "Rules file in the repo",
            Kind::Text,
            "With your rules in a git repository (Your CLAUDE.md, above), the file in it that holds them (CLAUDE.md when not set); setup writes ~/.claude/CLAUDE.md again when it changes, and the repository is pulled each time setup runs, an update's included",
        )],
    },
    OwnFile {
        file: "CLAUDE.md",
        category: "Your files",
        rows: &[whole(
            "Your CLAUDE.md rules",
            "Your own rules for Claude, when Your CLAUDE.md reads them from this local file",
        )],
    },
    OwnFile {
        file: "linear.json",
        category: "Linear (linear.json)",
        rows: &[
            row(
                "linearWorkspace",
                "Workspace",
                Kind::Text,
                "Your Linear workspace's name in its URLs (linear.app/<workspace>), for ticket links",
            ),
            row(
                "linearTeam",
                "Team",
                Kind::Text,
                "The team key /shape files tickets in (ABC)",
            ),
            row(
                "linearLabels",
                "Labels",
                Kind::List,
                "The labels /shape puts on a new ticket",
            ),
        ],
    },
    OwnFile {
        file: "browser.json",
        category: "Browser (browser.json)",
        rows: &[
            row(
                "hosts",
                "Chrome profiles",
                Kind::Map,
                "The Chrome profile each site's links open in (site=Profile 2, the profile's folder as chrome://version names it); any other site opens in the default profile",
            ),
            row(
                "default",
                "Default Chrome profile",
                Kind::Profile,
                "The Chrome profile a link opens in when Chrome profiles names none for its site; unset, the default browser",
            ),
        ],
    },
    OwnFile {
        file: "slack-updates.json",
        category: "Slack (slack-updates.json)",
        rows: &[row(
            "channels",
            "Channels",
            Kind::List,
            "The channel IDs whose every message the Slack view shows, besides your DMs, mentions and threads",
        )],
    },
    OwnFile {
        file: "slack-relay.json",
        category: "Slack relay (slack-relay.json)",
        rows: &[row(
            "owner",
            "Your member ID",
            Kind::Text,
            "Your Slack member ID, the one person the relay takes messages from; read when the relay starts",
        )],
    },
    OwnFile {
        file: "oncall.json",
        category: "On call (oncall.json)",
        rows: &[
            whole(
                "Alert channels",
                "The channels /oncall watches, and what in each is urgent (opens the file)",
            ),
            row(
                "rules",
                "Rules file",
                Kind::Text,
                "The Markdown file with your team's own on-call rules",
            ),
            row(
                "repo",
                "Repository",
                Kind::Text,
                "The GitHub repository (owner/name) runbooks, workflow runs and code are looked up in",
            ),
            row(
                "from",
                "Notices from",
                Kind::Text,
                "The name shown above each notice (oncall when not set)",
            ),
            row(
                "addressed",
                "Handled reactions",
                Kind::List,
                "Your team's reaction names (no colons) for an alert that is handled",
            ),
            row(
                "taking",
                "Taking reactions",
                Kind::List,
                "Your team's reaction names for an alert someone is on",
            ),
        ],
    },
    OwnFile {
        file: "sidebar.json",
        category: "Sidebar layout (sidebar.json)",
        rows: &[
            number(
                "width",
                "Sidebar width",
                20,
                "The columns the sidebar takes, 20 or more",
            ),
            number(
                "minWindowWidth",
                "Narrowest window",
                40,
                "The narrowest window that gets a sidebar, 40 or more",
            ),
            number(
                "cardWidth",
                "Card width",
                30,
                "Mission control's cards, in columns, 30 or more",
            ),
            number(
                "cardHeight",
                "Card height",
                6,
                "Mission control's cards, in lines, 6 or more",
            ),
            whole(
                "Plugins",
                "The plugins that load and their options, under \"plugins\" (opens the file)",
            ),
        ],
    },
    OwnFile {
        file: "keys.json",
        category: "Your files",
        rows: &[whole(
            "Key bindings",
            "Keys bound to other actions, or unbound, over Switchboard's own",
        )],
    },
    OwnFile {
        file: "trusted-dirs",
        category: "Your files",
        rows: &[whole(
            "Trusted directories",
            "Directories whose own scripts Switchboard may run, one a line",
        )],
    },
    OwnFile {
        file: "oncall-rules.md",
        category: "Your files",
        rows: &[whole(
            "On-call rules",
            "Your team's own on-call rules, when oncall.json's rules names this file",
        )],
    },
];

/// One of Chrome's profiles: its folder, which browser.json keeps, and the
/// name Chrome shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub dir: String,
    pub name: String,
}

/// Chrome's profiles, as its Local State lists them, by folder in natural
/// order (`Profile 2` before `Profile 10`); `None` when Chrome has no Local
/// State, as when it isn't installed.
pub fn chrome_profiles() -> Option<Vec<Profile>> {
    profiles_in(&platform::chrome_local_state()?)
}

/// The profiles a Local State file lists.
pub fn profiles_in(local_state: &Path) -> Option<Vec<Profile>> {
    let local = util::read_json(local_state)?;
    let cache = local
        .pointer("/profile/info_cache")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut list: Vec<Profile> = cache
        .iter()
        .map(|(dir, info)| Profile {
            dir: dir.clone(),
            name: info
                .get("name")
                .and_then(Value::as_str)
                .filter(|n| !n.is_empty())
                .unwrap_or(dir)
                .to_string(),
        })
        .collect();
    list.sort_by(|a, b| natural(&a.dir, &b.dir));
    Some(list)
}

/// Compares with runs of digits as numbers, as `localeCompare(…, { numeric:
/// true })` did for the folders.
fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    fn parts(s: &str) -> Vec<(bool, String)> {
        let mut out: Vec<(bool, String)> = Vec::new();
        for c in s.chars() {
            let digit = c.is_ascii_digit();
            match out.last_mut() {
                Some((d, run)) if *d == digit => run.push(c),
                _ => out.push((digit, c.to_string())),
            }
        }
        out
    }
    let (pa, pb) = (parts(a), parts(b));
    for (x, y) in pa.iter().zip(&pb) {
        let ord = match (x.0, y.0) {
            (true, true) => {
                let (nx, ny) = (x.1.trim_start_matches('0'), y.1.trim_start_matches('0'));
                nx.len().cmp(&ny.len()).then_with(|| nx.cmp(ny))
            }
            _ => x.1.to_lowercase().cmp(&y.1.to_lowercase()),
        };
        if ord.is_ne() {
            return ord;
        }
    }
    pa.len().cmp(&pb.len())
}

/// A JSON file of yours: a file that isn't there is empty; one that is
/// there but can't be read or parsed is an error, so nothing writes over it.
pub fn read_own(dir: &Path, file: &str) -> Result<Map<String, Value>> {
    let full = dir.join(file);
    let link = fs::symlink_metadata(&full).is_ok_and(|m| m.file_type().is_symlink());
    if link && !full.exists() {
        return Err(anyhow!("it is a link to nothing"));
    }
    let text = match fs::read_to_string(&full) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(e) => return Err(e.into()),
    };
    match serde_json::from_str::<Value>(&text)? {
        Value::Object(map) => Ok(map),
        _ => Err(anyhow!("it isn't a JSON object")),
    }
}

/// A value as it is typed in: what the question starts with, and what the
/// row shows in its detail.
pub fn typed_of(kind: Kind, value: Option<&Value>) -> String {
    let Some(value) = value.filter(|v| !v.is_null()) else {
        return String::new();
    };
    match (kind, value) {
        (Kind::List, Value::Array(list)) => list
            .iter()
            .map(util::js_string)
            .collect::<Vec<_>>()
            .join(", "),
        (Kind::Map, Value::Object(map)) => map
            .iter()
            .map(|(k, v)| format!("{k}={}", util::js_string(v)))
            .collect::<Vec<_>>()
            .join(", "),
        _ => util::js_string(value),
    }
}

/// What a typed value becomes in the file.
#[derive(Clone, Debug, PartialEq)]
pub enum Typed {
    /// Nothing typed: the key goes.
    Remove,
    Value(Value),
    /// What is wrong with it, in words.
    Error(String),
}

/// What is typed in, as the file keeps it.
pub fn parse_typed(kind: Kind, text: &str, min: i64) -> Typed {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Typed::Remove;
    }
    match kind {
        Kind::Number => match trimmed.parse::<i64>() {
            // Columns and lines, whole.
            Ok(n) if n >= min => Typed::Value(Value::from(n)),
            _ => Typed::Error(format!("{trimmed} isn't a whole number, {min} or more")),
        },
        Kind::List => Typed::Value(Value::Array(
            trimmed
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(Value::from)
                .collect(),
        )),
        Kind::Map => {
            let pairs: Vec<&str> = trimmed
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect();
            let mut map = Map::new();
            for pair in pairs {
                match pair.split_once('=') {
                    Some((k, v)) if !k.trim().is_empty() && !v.trim().is_empty() => {
                        map.insert(k.trim().to_string(), Value::from(v.trim()));
                    }
                    _ => return Typed::Error(format!("\"{pair}\" isn't site=profile")),
                }
            }
            Typed::Value(Value::Object(map))
        }
        _ => Typed::Value(Value::from(trimmed)),
    }
}

/// Sets one key of a JSON file of yours, keeping the rest, written into the
/// file a link points to (keeping the link) through a temporary file renamed
/// into place, with the file's own mode (a file holding a key may be 600; a
/// new one is only yours). Gives what is wrong, in words.
pub fn write_own(
    dir: &Path,
    file: &str,
    key: &str,
    kind: Kind,
    text: &str,
    min: i64,
) -> Result<(), String> {
    let parsed = parse_typed(kind, text, min);
    if let Typed::Error(e) = parsed {
        return Err(e);
    }
    let mut data = read_own(dir, file)
        .map_err(|e| format!("~/.config/switchboard/{file} can't be read ({e}); fix it first"))?;
    let full = dir.join(file);
    let full = fs::canonicalize(&full).unwrap_or(full);
    match parsed {
        Typed::Remove => {
            data.shift_remove(key);
        }
        Typed::Value(v) => {
            data.insert(key.to_string(), v);
        }
        Typed::Error(_) => unreachable!("returned above"),
    }
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(&full).map_or(0o600, |m| m.permissions().mode() & 0o777)
    };
    let write = || -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        fs::create_dir_all(full.parent().unwrap_or(dir))?;
        let tmp = PathBuf::from(format!("{}.tmp", full.display()));
        fs::write(
            &tmp,
            format!("{}\n", util::json_pretty(&Value::Object(data))),
        )?;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))?;
        fs::rename(&tmp, &full)
    };
    write().map_err(|e| format!("Couldn't write ~/.config/switchboard/{file}: {e}"))
}

/// A row's short value, shown beside it.
fn short_of(
    kind: Kind,
    value: Option<&Value>,
    exists: bool,
    profiles: &Option<Vec<Profile>>,
) -> String {
    if kind == Kind::File {
        return if exists { "edit" } else { "not there" }.into();
    }
    let Some(value) = value.filter(|v| !v.is_null()) else {
        return "not set".into();
    };
    match (kind, value) {
        (_, Value::Array(list)) if list.is_empty() => "not set".into(),
        (Kind::List, Value::Array(list)) => format!("{} set", list.len()),
        (Kind::List, _) => "1 set".into(),
        (Kind::Map, Value::Object(map)) => format!("{} set", map.len()),
        (Kind::Profile, v) => {
            let dir = util::js_string(v);
            profiles
                .as_ref()
                .and_then(|list| list.iter().find(|p| p.dir == dir))
                .map_or(dir, |p| p.name.clone())
        }
        (_, v) => util::js_string(v),
    }
}

/// The Settings view's rows for your own files, in `OWN_FILES`' order of
/// categories, each row under its category (`group`). Its data says what
/// Enter does: `own` holds the file, key, kind, least number, label and
/// what is typed now, and `value` the short value shown beside it.
pub fn own_items(dir: &Path) -> Vec<Item> {
    let profiles = chrome_profiles();
    items_with(dir, &profiles)
}

fn items_with(dir: &Path, profiles: &Option<Vec<Profile>>) -> Vec<Item> {
    let mut categories: Vec<&str> = Vec::new();
    for f in OWN_FILES {
        if !categories.contains(&f.category) {
            categories.push(f.category);
        }
    }
    let mut items = Vec::new();
    for category in categories {
        for f in OWN_FILES.iter().filter(|f| f.category == category) {
            let (data, broken) = if f.file.ends_with(".json") {
                match read_own(dir, f.file) {
                    Ok(d) => (d, None),
                    Err(e) => (Map::new(), Some(e.to_string())),
                }
            } else {
                (Map::new(), None)
            };
            let exists = dir.join(f.file).exists();
            for r in f.rows {
                let value = r.key.and_then(|k| data.get(k));
                // A list with a comma inside one of its entries, or a map with
                // one in a key or value, can't be typed back as it is: it
                // opens in the editor.
                let tangled = match (r.kind, value) {
                    (Kind::List, Some(Value::Array(list))) => {
                        list.iter().any(|v| util::js_string(v).contains(','))
                    }
                    (Kind::Map, Some(Value::Object(map))) => map.iter().any(|(k, v)| {
                        format!("{k}{}", util::js_string(v)).contains(',') || k.contains('=')
                    }),
                    _ => false,
                };
                let kind = if tangled { Kind::File } else { r.kind };
                let typed = typed_of(r.kind, value);
                let detail = match &broken {
                    Some(why) => format!("~/.config/switchboard/{} can't be read: {why}", f.file),
                    None if !typed.is_empty() => format!("{}: {typed}", r.detail),
                    None => format!("{} (~/.config/switchboard/{})", r.detail, f.file),
                };
                let short = if broken.is_some() {
                    "unreadable".to_string()
                } else {
                    short_of(r.kind, value, exists, profiles)
                };
                let active = !typed.is_empty() || (r.kind == Kind::File && exists);
                items.push(
                    Item::new(format!("own:{}:{}", f.file, r.key.unwrap_or("")), r.label)
                        .detail(detail)
                        .group(category)
                        .state(if active { State::Active } else { State::Idle })
                        .data(json!({
                            "value": short,
                            "own": {
                                "file": f.file,
                                "key": r.key,
                                "kind": kind.name(),
                                "min": r.min,
                                "label": r.label,
                                "typed": typed,
                            },
                            "search": format!("{category} {} {} {typed}", f.file, r.key.unwrap_or("")),
                        })),
                );
            }
        }
    }
    items
}

/// Your own files' directory.
pub fn own_dir() -> PathBuf {
    paths::config_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sb-own-{name}-{}-{}",
            std::process::id(),
            util::now_ms()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn row<'a>(items: &'a [Item], id: &str) -> &'a Item {
        items
            .iter()
            .find(|i| i.id == id)
            .unwrap_or_else(|| panic!("no {id}"))
    }

    #[test]
    fn rows_are_listed_under_each_files_name_and_typing_one_keeps_the_rest() {
        let dir = temp("rows");
        fs::write(
            dir.join("linear.json"),
            "{\"linearTeam\":\"ABC\",\"apiKeyNote\":\"kept\"}",
        )
        .unwrap();
        fs::write(dir.join("oncall.json"), "not json").unwrap();
        fs::write(
            dir.join("browser.json"),
            "{\"hosts\":{\"a.com\":\"Profile 2\"},\"default\":\"Profile 1\"}",
        )
        .unwrap();
        let profiles = Some(vec![Profile {
            dir: "Profile 1".into(),
            name: "Work".into(),
        }]);
        let items = items_with(&dir, &profiles);
        let team = row(&items, "own:linear.json:linearTeam");
        assert_eq!(team.group.as_deref(), Some("Linear (linear.json)"));
        assert_eq!(team.data["value"], "ABC");
        assert_eq!(team.state, State::Active);
        assert!(team.detail.ends_with(": ABC"));
        let labels = row(&items, "own:linear.json:linearLabels");
        assert_eq!(labels.data["value"], "not set");
        assert!(
            labels
                .detail
                .ends_with("(~/.config/switchboard/linear.json)")
        );
        let rules = row(&items, "own:oncall.json:rules");
        assert_eq!(rules.data["value"], "unreadable");
        assert!(rules.detail.contains("can't be read"));
        assert_eq!(row(&items, "own:browser.json:hosts").data["value"], "1 set");
        assert_eq!(
            row(&items, "own:browser.json:default").data["value"],
            "Work",
            "a profile by the name Chrome shows"
        );
        assert_eq!(row(&items, "own:trusted-dirs:").data["value"], "not there");
        assert_eq!(row(&items, "own:trusted-dirs:").data["own"]["kind"], "file");

        write_own(
            &dir,
            "linear.json",
            "linearLabels",
            Kind::List,
            " bug, ui ,",
            1,
        )
        .unwrap();
        let saved = util::read_json(dir.join("linear.json")).unwrap();
        assert_eq!(
            saved,
            json!({ "linearTeam": "ABC", "apiKeyNote": "kept", "linearLabels": ["bug", "ui"] })
        );
        write_own(&dir, "linear.json", "linearTeam", Kind::Text, "  ", 1).unwrap();
        assert!(
            util::read_json(dir.join("linear.json"))
                .unwrap()
                .get("linearTeam")
                .is_none()
        );
        assert!(
            write_own(&dir, "oncall.json", "rules", Kind::Text, "x.md", 1)
                .unwrap_err()
                .contains("fix it first"),
            "a file that can't be read isn't written over"
        );
        assert_eq!(
            fs::read_to_string(dir.join("oncall.json")).unwrap(),
            "not json"
        );
        assert_eq!(
            write_own(&dir, "sidebar.json", "width", Kind::Number, "12", 20),
            Err("12 isn't a whole number, 20 or more".to_string())
        );
        write_own(&dir, "sidebar.json", "width", Kind::Number, "32", 20).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.join("sidebar.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "a new file is only yours");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn typed_values_read_back_as_they_were_typed() {
        assert_eq!(typed_of(Kind::List, Some(&json!(["a", "b"]))), "a, b");
        assert_eq!(
            typed_of(Kind::Map, Some(&json!({ "a.com": "P 2" }))),
            "a.com=P 2"
        );
        assert_eq!(typed_of(Kind::Number, Some(&json!(30))), "30");
        assert_eq!(typed_of(Kind::Text, None), "");
        assert_eq!(
            parse_typed(Kind::Map, "a.com=Profile 2, b", 1),
            Typed::Error("\"b\" isn't site=profile".into())
        );
        assert_eq!(
            parse_typed(Kind::Map, "a.com = Profile 2", 1),
            Typed::Value(json!({ "a.com": "Profile 2" }))
        );
        assert_eq!(
            parse_typed(Kind::Number, "3.5", 1),
            Typed::Error("3.5 isn't a whole number, 1 or more".into())
        );
    }

    #[test]
    fn chromes_profiles_come_in_their_folders_order() {
        let dir = temp("chrome");
        let file = dir.join("Local State");
        fs::write(
            &file,
            json!({ "profile": { "info_cache": {
                "Profile 10": { "name": "Ten" },
                "Default": { "name": "Me" },
                "Profile 2": { "name": "" },
            } } })
            .to_string(),
        )
        .unwrap();
        let list = profiles_in(&file).unwrap();
        assert_eq!(
            list.iter()
                .map(|p| (p.dir.as_str(), p.name.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("Default", "Me"),
                ("Profile 2", "Profile 2"),
                ("Profile 10", "Ten")
            ]
        );
        assert_eq!(profiles_in(&dir.join("none")), None);
        let _ = fs::remove_dir_all(&dir);
    }
}
