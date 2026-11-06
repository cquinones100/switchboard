//! Plugins bring in things from outside the workspace, from any service with
//! an API: a plugin fetches a list of items, which mission control shows as
//! a view of its own and the sidebar, if the plugin asks, as a short list
//! under the tabs. Ported from lib/sidebar/plugins.js.
//!
//! A plugin was a CommonJS module the daemon loaded; with no Node, it is now
//! an executable, `~/.config/switchboard/sidebar-plugins/<name>`, in any
//! language. It is run on its interval, given its options as JSON on stdin,
//! and prints its items as JSON on stdout: a list of items, or an object
//! with `items` and, optionally, `title`, `sidebar` and `intervalSeconds`:
//!
//! ```json
//! { "title": "Deploys", "items": [
//!   { "id": "1", "title": "api", "detail": "rolling out", "state": "active",
//!     "url": "https://…", "time": 1762400000000 } ] }
//! ```
//!
//! `state` is `attention`, `active` or `idle`; Enter opens an item's `url`.
//! A run that takes over `TIMEOUT` is killed, and one that fails keeps the
//! items the last good one gave, with its error's first line shown above
//! them. Every run is off the server's task.
//!
//! Only plugins named in `~/.config/switchboard/sidebar.json`'s `plugins` are
//! loaded, each given what the config holds under its name as its options
//! (`true` for none, `false` to leave it out; `intervalSeconds` and `sidebar`
//! there win over the plugin's own). A name is looked for among your
//! executables first, then among the built-in ones (`github`, `github.rs`).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use serde_json::{Map, Value};
use tokio::io::AsyncWriteExt;

use crate::items::{Item, State, Target, ViewData};

/// How long a run may take.
pub const TIMEOUT: Duration = Duration::from_secs(30);
/// How often a plugin runs when neither it nor its options say.
pub const INTERVAL: u64 = 300;
/// Output past this is cut off, as the daemon's `maxBuffer` did.
const MAX_OUTPUT: usize = 16 * 1024 * 1024;

/// Where a plugin comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// An executable of yours.
    Exec(PathBuf),
    /// One built in (`github`).
    Builtin(&'static str),
}

/// The built-in plugins, by name, and the title each gives its view.
pub const BUILT_IN: &[(&str, &str)] = &[("github", super::github_plugin::TITLE)];

/// One loaded plugin and what it last gave.
#[derive(Clone, Debug)]
pub struct Plugin {
    pub name: String,
    pub source: Source,
    pub title: String,
    pub interval: Duration,
    /// Items shown in the sidebar (0: none).
    pub sidebar: usize,
    pub options: Value,
    /// Whether the config set the title, interval or sidebar count, which
    /// then win over what the plugin prints.
    set_title: bool,
    set_interval: bool,
    set_sidebar: bool,
    pub items: Vec<Item>,
    pub loaded: bool,
    pub error: Option<String>,
    pub fetching: bool,
    pub fetched_at: Option<Instant>,
    /// When its last run ended, in milliseconds since the epoch.
    pub at: Option<i64>,
}

impl Plugin {
    /// Whether its next run is due.
    pub fn due(&self, now: Instant) -> bool {
        !self.fetching
            && self
                .fetched_at
                .is_none_or(|at| now.duration_since(at) >= self.interval)
    }

    /// Its view.
    pub fn view(&self) -> ViewData {
        ViewData {
            items: self.items.clone(),
            error: self.error.clone(),
            at: self.at,
            ..ViewData::default()
        }
    }

    /// Takes in a run's answer: new items, or the error kept beside the
    /// items it had.
    pub fn answered(&mut self, result: Result<Output, String>, now: Instant, at: i64) {
        match result {
            Ok(out) => {
                self.items = out.items;
                self.error = None;
                if let Some(t) = out.title.filter(|_| !self.set_title) {
                    self.title = t;
                }
                if let Some(s) = out.sidebar.filter(|_| !self.set_sidebar) {
                    self.sidebar = s;
                }
                if let Some(i) = out.interval.filter(|_| !self.set_interval) {
                    self.interval = Duration::from_secs(i.max(1));
                }
            }
            Err(e) => self.error = Some(e.lines().next().unwrap_or("").to_string()),
        }
        self.loaded = true;
        self.fetching = false;
        self.fetched_at = Some(now);
        self.at = Some(at);
    }
}

/// Whether a name may be a plugin's: no path in it.
fn good_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The plugins sidebar.json names, from `user_dir`, else built in, and what
/// couldn't be loaded, in words for the server's log.
pub fn load(config: &Value, user_dir: &Path) -> (Vec<Plugin>, Vec<String>) {
    let mut plugins = Vec::new();
    let mut problems = Vec::new();
    let Some(named) = config.get("plugins").and_then(Value::as_object) else {
        return (plugins, problems);
    };
    for (name, options) in named {
        if !good_name(name) || *options == Value::Bool(false) {
            continue;
        }
        let own = user_dir.join(name);
        let source = if is_executable(&own) {
            Source::Exec(own)
        } else if let Some((builtin, _)) = BUILT_IN.iter().find(|(n, _)| n == name) {
            Source::Builtin(builtin)
        } else {
            problems.push(format!(
                "plugin {name}: no executable {name} in {}, and none built in",
                crate::paths::tilde(user_dir)
            ));
            continue;
        };
        let options = match options {
            Value::Object(map) => Value::Object(map.clone()),
            _ => Value::Object(Map::new()),
        };
        let builtin_title = match &source {
            Source::Builtin(n) => BUILT_IN.iter().find(|(b, _)| b == n).map(|(_, t)| *t),
            Source::Exec(_) => None,
        };
        let title = options.get("title").and_then(Value::as_str);
        let interval = options.get("intervalSeconds").and_then(Value::as_u64);
        let sidebar = options.get("sidebar").and_then(Value::as_u64);
        plugins.push(Plugin {
            name: name.clone(),
            title: title
                .map(str::to_string)
                .or(builtin_title.map(str::to_string))
                .unwrap_or_else(|| name.clone()),
            interval: Duration::from_secs(interval.unwrap_or(INTERVAL).max(1)),
            sidebar: sidebar.map_or(
                if matches!(source, Source::Builtin(_)) {
                    3
                } else {
                    0
                },
                |s| s as usize,
            ),
            set_title: title.is_some(),
            set_interval: interval.is_some(),
            set_sidebar: sidebar.is_some(),
            source,
            options,
            items: Vec::new(),
            loaded: false,
            error: None,
            fetching: false,
            fetched_at: None,
            at: None,
        });
    }
    (plugins, problems)
}

/// What a run gave.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Output {
    pub items: Vec<Item>,
    pub title: Option<String>,
    pub sidebar: Option<usize>,
    pub interval: Option<u64>,
}

/// One item as a plugin prints it.
pub fn item_of(v: &Value, at: usize) -> Option<Item> {
    let title = crate::util::js_string(v.get("title")?);
    let id = match v.get("id") {
        Some(Value::Null) | None => v
            .get("url")
            .and_then(Value::as_str)
            .map_or_else(|| format!("{at}"), str::to_string),
        Some(id) => crate::util::js_string(id),
    };
    let state = match v.get("state").and_then(Value::as_str) {
        Some("attention") => State::Attention,
        Some("active") => State::Active,
        _ => State::Idle,
    };
    let mut item = Item::new(id, title).state(state);
    if let Some(d) = v.get("detail").filter(|d| !d.is_null()) {
        item.detail = crate::util::js_string(d);
    }
    // Only a web link opens, as the daemon opened only http(s).
    if let Some(url) = v
        .get("url")
        .and_then(Value::as_str)
        .filter(|u| u.starts_with("https://") || u.starts_with("http://"))
    {
        item.target = Target::Url(url.to_string());
    }
    if let Some(t) = v
        .get("time")
        .and_then(Value::as_f64)
        .filter(|t| t.is_finite())
    {
        item.time = Some(t as i64);
    }
    item.data = v.clone();
    Some(item)
}

/// A run's stdout read as its items.
pub fn parse_output(stdout: &str) -> Result<Output> {
    let value: Value =
        serde_json::from_str(stdout.trim()).map_err(|e| anyhow!("its output isn't JSON: {e}"))?;
    let (list, rest) = match &value {
        Value::Array(list) => (list.clone(), None),
        Value::Object(map) => (
            map.get("items")
                .and_then(Value::as_array)
                .cloned()
                .ok_or_else(|| anyhow!("its output has no \"items\" list"))?,
            Some(map),
        ),
        _ => return Err(anyhow!("its output is neither a list nor an object")),
    };
    Ok(Output {
        items: list
            .iter()
            .enumerate()
            .filter_map(|(i, v)| item_of(v, i))
            .collect(),
        title: rest
            .and_then(|m| m.get("title"))
            .and_then(Value::as_str)
            .map(str::to_string),
        sidebar: rest
            .and_then(|m| m.get("sidebar"))
            .and_then(Value::as_u64)
            .map(|s| s as usize),
        interval: rest
            .and_then(|m| m.get("intervalSeconds"))
            .and_then(Value::as_u64),
    })
}

/// Runs an executable plugin: its options on stdin, its output read as
/// items, killed after `timeout`. Never through a shell.
pub async fn run_exec(path: &Path, options: &Value, timeout: Duration) -> Result<Output> {
    let mut child = tokio::process::Command::new(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| anyhow!("couldn't start {}: {e}", crate::paths::tilde(path)))?;
    if let Some(mut stdin) = child.stdin.take() {
        let input = crate::util::json_compact(options);
        // A plugin that exits without reading its options is not an error.
        let _ = stdin.write_all(input.as_bytes()).await;
        let _ = stdin.shutdown().await;
    }
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| anyhow!("it took over {}s", timeout.as_secs()))??;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let first = stderr
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim();
        let how = match output.status.code() {
            Some(code) => format!("it exited {code}"),
            None => "it was killed".to_string(),
        };
        return Err(if first.is_empty() {
            anyhow!("{how}")
        } else {
            anyhow!("{how}: {first}")
        });
    }
    let stdout = &output.stdout[..output.stdout.len().min(MAX_OUTPUT)];
    parse_output(&String::from_utf8_lossy(stdout))
}

/// Runs a plugin, whichever kind.
pub async fn run(source: Source, options: Value) -> Result<Output, String> {
    let result = match source {
        Source::Exec(path) => run_exec(&path, &options, TIMEOUT).await,
        Source::Builtin(name) => match name {
            "github" => tokio::time::timeout(TIMEOUT, super::github_plugin::fetch(&options))
                .await
                .map_err(|_| anyhow!("it took over {}s", TIMEOUT.as_secs()))
                .and_then(|r| r)
                .map(|items| Output {
                    items,
                    ..Output::default()
                }),
            other => Err(anyhow!("no built-in plugin {other}")),
        },
    };
    result.map_err(|e| format!("{e:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sb-plugins-{name}-{}-{}",
            std::process::id(),
            crate::util::now_ms()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn script(dir: &Path, name: &str, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn only_named_plugins_load_and_a_bad_name_or_a_missing_one_doesnt() {
        let dir = temp("load");
        script(&dir, "demo", "cat");
        std::fs::write(dir.join("plain"), "not executable").unwrap();
        let config = json!({ "plugins": {
            "demo": { "word": "hello", "sidebar": 2 },
            "../evil": {},
            "missing": {},
            "plain": true,
            "github": true,
            "off": false,
        } });
        let (plugins, problems) = load(&config, &dir);
        assert_eq!(
            plugins.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec!["demo", "github"]
        );
        assert_eq!(plugins[0].source, Source::Exec(dir.join("demo")));
        assert_eq!(plugins[0].sidebar, 2);
        assert_eq!(plugins[0].options, json!({ "word": "hello", "sidebar": 2 }));
        assert_eq!(plugins[1].source, Source::Builtin("github"));
        assert_eq!(plugins[1].title, "Pull requests");
        assert!(problems.iter().any(|p| p.contains("missing")));
        assert!(
            problems.iter().any(|p| p.contains("plain")),
            "a file that isn't executable isn't run"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_plugins_items_are_read_and_kept_when_a_later_run_fails() {
        let dir = temp("run");
        // Its options arrive on stdin; the first run answers, the next fails.
        script(
            &dir,
            "demo",
            r#"read options
if [ -e "$(dirname "$0")/ran" ]; then echo "service down" >&2; echo stack >&2; exit 3; fi
touch "$(dirname "$0")/ran"
word=$(printf '%s' "$options" | sed 's/.*"word":"\([^"]*\)".*/\1/')
printf '{"title":"Demo","sidebar":2,"items":[{"id":1,"title":"first %s","state":"attention"},{"id":2,"title":"second","state":"idle","url":"https://example.com/2"}]}' "$word""#,
        );
        let (mut plugins, _) = load(&json!({ "plugins": { "demo": { "word": "hello" } } }), &dir);
        let plugin = &mut plugins[0];
        assert!(plugin.due(Instant::now()));
        let first = crate::util::block_on(run(plugin.source.clone(), plugin.options.clone()));
        plugin.answered(first, Instant::now(), 1);
        assert_eq!(plugin.items[0].title, "first hello");
        assert_eq!(plugin.items[0].id, "1");
        assert_eq!(plugin.items[0].state, State::Attention);
        assert_eq!(
            plugin.items[1].target,
            Target::Url("https://example.com/2".into())
        );
        assert_eq!(plugin.title, "Demo");
        assert_eq!(plugin.sidebar, 2);
        assert!(!plugin.due(Instant::now()), "not again before its interval");
        let second = crate::util::block_on(run(plugin.source.clone(), plugin.options.clone()));
        plugin.answered(second, Instant::now(), 2);
        assert_eq!(plugin.error.as_deref(), Some("it exited 3: service down"));
        assert_eq!(plugin.items.len(), 2, "the items it had are kept");
        let view = plugin.view();
        assert_eq!(view.error.as_deref(), Some("it exited 3: service down"));
        assert_eq!(view.at, Some(2));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_plugin_that_takes_too_long_is_given_up_on() {
        let dir = temp("slow");
        script(&dir, "slow", "sleep 5");
        let started = Instant::now();
        let result = crate::util::block_on(run_exec(
            &dir.join("slow"),
            &json!({}),
            Duration::from_millis(200),
        ));
        assert!(result.unwrap_err().to_string().contains("took over"));
        assert!(started.elapsed() < Duration::from_secs(4));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn output_is_a_list_or_an_object_with_items() {
        let out =
            parse_output(r#"[{"title":"a","time":5,"url":"file:///x"},{"no":"title"}]"#).unwrap();
        assert_eq!(out.items.len(), 1);
        assert_eq!(out.items[0].id, "file:///x");
        assert_eq!(out.items[0].time, Some(5));
        assert_eq!(out.items[0].target, Target::None, "only a web link opens");
        assert!(parse_output("{}").is_err());
        assert!(parse_output("nope").is_err());
    }
}
