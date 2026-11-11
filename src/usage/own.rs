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
    /// Where your CLAUDE.md's rules come from (claude.json's `repo` or
    /// `path`, else the local file), picked from a menu, the file or
    /// repository typed in.
    Rules,
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
            Kind::Rules => "rules",
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
        rows: &[
            Row {
                key: None,
                label: "Your CLAUDE.md",
                kind: Kind::Rules,
                min: None,
                detail: "Your own rules, which ~/.claude/CLAUDE.md imports",
            },
            row(
                "file",
                "Rules file in the repo",
                Kind::Text,
                "With your rules in a git repository (claude.json's repo), the file in it that holds them (CLAUDE.md when not set); saving it runs setup's claude-md part, which writes ~/.claude/CLAUDE.md again, and the repository is pulled each time that part runs",
            ),
        ],
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
                "The Chrome profile each site's links open in (site=Profile 2, the profile's folder as chrome://version names it), awsapps.com for the palette's AWS login; any other site opens in the default profile",
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
        file: "repos.json",
        category: "Your files",
        rows: &[whole(
            "Repositories",
            "Each repository's own settings by its origin's owner/name, such as the apply list of what a landed change needs to take effect",
        )],
    },
    OwnFile {
        file: "leak-check.txt",
        category: "Your files",
        rows: &[whole(
            "Identifying words",
            "Your organization's and company's names, usernames and internal hosts, one a line, which leak-check finds in what a landing would push to a public repository",
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

/// Where your CLAUDE.md's rules come from, as claude.json says: its kind
/// ("repository", "a file" or "local file", as the Settings row shows it)
/// and the words that name it. Ported from views.js's `claudeMdSource`.
pub fn rules_source(dir: &Path, data: &Map<String, Value>) -> (&'static str, String) {
    let text = |k: &str| {
        data.get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    if let Some(repo) = text("repo") {
        return ("repository", format!("repo: {repo}"));
    }
    if let Some(path) = text("path") {
        return ("a file", path);
    }
    let local = if dir.join("CLAUDE.md").exists() {
        "~/.config/switchboard/CLAUDE.md"
    } else {
        "none yet"
    };
    ("local file", local.to_string())
}

/// Where your rules come from, written to claude.json: a repository, a
/// file, or (neither) the local file, the other key taken out.
pub fn write_rules(dir: &Path, repo: Option<&str>, path: Option<&str>) -> Result<(), String> {
    write_own(
        dir,
        "claude.json",
        "repo",
        Kind::Text,
        repo.unwrap_or(""),
        0,
    )?;
    write_own(
        dir,
        "claude.json",
        "path",
        Kind::Text,
        path.unwrap_or(""),
        0,
    )
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

/// Settings' words for how a repository's work lands.
pub fn landing_words(method: &str) -> &'static str {
    match method {
        "pr" => "through a PR",
        "direct" => "straight to the branch",
        _ => "automatic",
    }
}

/// The category Settings lists each repository's landing under.
pub const LANDING: &str = "Landing (repos.json)";

/// Settings' rows for `~/.config/switchboard/repos.json` (src/repos.rs), as
/// own-config.js's `landingItems` gave them: for each repository a tab is
/// in, how its work lands (automatic, through a PR, or straight on its
/// default branch), and, landing straight, its steps. The automatic row
/// says what the default found, as last worked out. Each row's
/// `repo_setting` says what Enter changes.
pub fn landing_items(repos: &[String]) -> Vec<Item> {
    let cache = crate::repos::cache_file();
    landing_rows(repos, crate::repos::read(), &|repo| {
        crate::repos::cached_in(&cache, repo)
    })
}

/// `landing_items` from the file as read and the answers worked out.
pub fn landing_rows(
    repos: &[String],
    data: Result<Map<String, Value>, String>,
    cached: &dyn Fn(&str) -> Option<crate::repos::Cached>,
) -> Vec<Item> {
    let mut list: Vec<&String> = Vec::new();
    for r in repos {
        if !list.iter().any(|l| l.eq_ignore_ascii_case(r)) {
            list.push(r);
        }
    }
    list.sort();
    let (data, broken) = match data {
        Ok(d) => (d, None),
        Err(e) => (Map::new(), Some(e)),
    };
    let mut items = Vec::new();
    for repo in list {
        let entry = crate::repos::landing_entry(&data, repo);
        let set = entry.get("landing").and_then(Value::as_str);
        let worked = cached(repo);
        let method = set.or(worked.as_ref().map(|c| c.landing.as_str()));
        let (detail, value) = match (&broken, set, &worked) {
            (Some(e), _, _) => (
                format!("~/.config/switchboard/repos.json can't be read: {e}"),
                "unreadable".to_string(),
            ),
            (None, Some(m), _) => (
                "How its work lands, set in repos.json".to_string(),
                landing_words(m).to_string(),
            ),
            (None, None, Some(c)) => (
                format!("Worked out: {}, since {}", landing_words(&c.landing), c.why),
                format!("auto: {}", c.landing),
            ),
            (None, None, None) => (
                "Worked out the first time it lands, or when you come to Settings".to_string(),
                "auto: ?".to_string(),
            ),
        };
        let row = |field: &str, title: String, value: String, detail: String, now: Value| {
            Item::new(format!("repo:{repo}:{field}"), title)
                .detail(detail)
                .group(LANDING)
                .state(if field != "landing" || set.is_some() {
                    State::Active
                } else {
                    State::Idle
                })
                .data(json!({
                    "value": value,
                    "repo_setting": { "repo": repo, "field": field, "value": now },
                    "search": format!("Landing repos.json {repo} {field} {value}"),
                }))
        };
        items.push(row(
            "landing",
            repo.clone(),
            value,
            detail,
            json!(set.unwrap_or("automatic")),
        ));
        if method != Some("direct") || broken.is_some() {
            continue;
        }
        let steps = crate::repos::steps_of(&entry);
        let (test_value, test_detail) = match steps.test.as_deref() {
            None => (
                "not set".to_string(),
                "The command run before the review and after the last rebase; not set, a landing runs what its CLAUDE.md names".to_string(),
            ),
            Some("") => (
                "none".to_string(),
                "The command run before the review and after the last rebase; none".to_string(),
            ),
            Some(t) => (
                t.to_string(),
                format!("Run before the review and after the last rebase: {t}"),
            ),
        };
        items.push(row(
            "test",
            "  Tests".into(),
            test_value,
            test_detail,
            json!(steps.test),
        ));
        items.push(row(
            "testIn",
            "  Tests run".into(),
            steps.test_in.clone(),
            "here runs the tests in the worktree; ci pushes the landing's commit to a ci/ branch of its own, waits for the repository's CI on it and deletes the branch, running them here when no CI answers".into(),
            json!(steps.test_in),
        ));
        items.push(row(
            "squash",
            "  Squash".into(),
            if steps.squash { "on" } else { "off" }.into(),
            "Land the branch as one commit; off, its commits land as they are".into(),
            json!(steps.squash),
        ));
        items.push(row(
            "review",
            "  Review".into(),
            steps.review.clone(),
            "review-loop reviews it before the push; none doesn't".into(),
            json!(steps.review),
        ));
        items.push(row(
            "beforePush",
            "  Before push".into(),
            if steps.before_push.is_empty() {
                "none".into()
            } else {
                steps.before_push.clone()
            },
            if steps.before_push.is_empty() {
                "A command that must pass before the push; none set".into()
            } else {
                format!("Must pass before the push: {}", steps.before_push)
            },
            json!(steps.before_push),
        ));
    }
    items
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
                let (detail, short, typed, active) = if r.kind == Kind::Rules && broken.is_none() {
                    let (kind, words) = rules_source(dir, &data);
                    (
                        format!("{}: {words}", r.detail),
                        kind.to_string(),
                        words,
                        true,
                    )
                } else {
                    (detail, short, typed, active)
                };
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

    #[test]
    fn each_repository_s_landing_is_a_row_and_landing_straight_its_steps() {
        let data =
            json!({ "acme/direct": { "landing": "direct", "test": "npm test", "squash": false } });
        let worked = |repo: &str| {
            (repo == "acme/auto").then(|| crate::repos::Cached {
                landing: "pr".into(),
                why: "main has a merge queue".into(),
                at: 1,
            })
        };
        let repos = vec![
            "acme/auto".to_string(),
            "acme/direct".into(),
            "acme/new".into(),
            "acme/direct".into(),
        ];
        let rows = landing_rows(&repos, Ok(data.as_object().unwrap().clone()), &worked);
        let shown: Vec<(&str, &str)> = rows
            .iter()
            .map(|i| (i.title.as_str(), i.data["value"].as_str().unwrap_or("")))
            .collect();
        assert_eq!(
            shown,
            vec![
                ("acme/auto", "auto: pr"),
                ("acme/direct", "straight to the branch"),
                ("  Tests", "npm test"),
                ("  Tests run", "here"),
                ("  Squash", "off"),
                ("  Review", "review-loop"),
                ("  Before push", "none"),
                ("acme/new", "auto: ?"),
            ],
            "each repository once, its steps only when it lands straight"
        );
        assert_eq!(
            rows[0].detail,
            "Worked out: through a PR, since main has a merge queue"
        );
        assert_eq!(rows[0].data["repo_setting"]["value"], "automatic");
        assert_eq!(rows[3].data["repo_setting"]["field"], "testIn");
        assert_eq!(rows[4].data["repo_setting"]["field"], "squash");
        assert!(rows.iter().all(|r| r.group.as_deref() == Some(LANDING)));
        let broken = landing_rows(&repos[..1], Err("bad".into()), &worked);
        assert_eq!(broken[0].data["value"], "unreadable");
    }

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
    fn your_claude_md_row_says_where_the_rules_come_from_and_changing_it_keeps_the_rest() {
        let dir = temp("rules");
        let rules = |dir: &Path| {
            let items = items_with(dir, &None);
            let r = row(&items, "own:claude.json:");
            (r.data["value"].clone(), r.detail.clone())
        };
        assert_eq!(rules(&dir).0, "local file");
        assert!(rules(&dir).1.ends_with(": none yet"));
        fs::write(dir.join("CLAUDE.md"), "mine\n").unwrap();
        assert!(rules(&dir).1.ends_with(": ~/.config/switchboard/CLAUDE.md"));

        fs::write(
            dir.join("claude.json"),
            "{\"path\":\"~/a.md\",\"file\":\"R.md\"}",
        )
        .unwrap();
        write_rules(&dir, Some("https://example.com/r.git"), None).unwrap();
        let (value, detail) = rules(&dir);
        assert_eq!(value, "repository");
        assert!(
            detail.ends_with(": repo: https://example.com/r.git"),
            "{detail}"
        );
        let data = read_own(&dir, "claude.json").unwrap();
        assert_eq!(data.get("path"), None);
        assert_eq!(data["file"], "R.md");

        write_rules(&dir, None, Some("~/b.md")).unwrap();
        assert_eq!(rules(&dir).0, "a file");
        write_rules(&dir, None, None).unwrap();
        assert_eq!(rules(&dir).0, "local file");
        assert_eq!(read_own(&dir, "claude.json").unwrap().len(), 1);
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
        assert_eq!(
            row(&items, "own:leak-check.txt:").data["own"]["kind"],
            "file",
            "leak-check's word list, opened from Settings"
        );
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
