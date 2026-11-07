//! Your settings for each repository, in `~/.config/switchboard/repos.json`,
//! by its origin's `owner/name` on GitHub:
//!
//! ```json
//! { "owner/name": { "apply": [{ "paths": ["db/**"], "label": "Reload the dev server", "command": "make reload" }] } }
//! ```
//!
//! `apply` is what a change landed there needs to take effect, which
//! `switchboard apply-actions` lists in place of what it would work out by
//! itself. The file is yours and never in this repository. Ported from
//! bin/apply-actions' `entry` and lib/repos.js.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::integrations::github;
use crate::{paths, util, worktree};

/// `~/.config/switchboard/repos.json`.
pub fn file() -> PathBuf {
    paths::config_dir().join("repos.json")
}

/// `owner/name` of a git remote's URL on GitHub, as the URL has them, or
/// `None` for one elsewhere.
pub fn repo_of_url(url: &str) -> Option<String> {
    github::parse_remote(url).map(|r| format!("{}/{}", r.owner, r.name))
}

/// `owner/name` of the origin of the repository at `dir`, or `None` outside
/// one or with an origin not on GitHub. Runs git, so never on the drawing
/// path.
pub fn repo_at(dir: &Path) -> Option<String> {
    let url = worktree::git_at(dir, &["remote", "get-url", "origin"]).ok()?;
    repo_of_url(&url)
}

/// The file as it is: empty when it isn't there; an error, saying why, when
/// it can't be read as a JSON object, so nothing writes over it.
pub fn read() -> Result<Map<String, Value>, String> {
    let path = file();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return if path.exists() {
            Err("it can't be read".into())
        } else {
            Ok(Map::new())
        };
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err("it isn't a JSON object".into()),
        Err(e) => Err(e.to_string()),
    }
}

/// The file, or nothing when it can't be read.
pub fn read_safely() -> Map<String, Value> {
    read().unwrap_or_default()
}

/// A repository's entry, by its `owner/name`, which GitHub takes in any
/// case, so the file's may be written in another; empty when there is none.
pub fn entry_of(data: &Map<String, Value>, repo: &str) -> Map<String, Value> {
    let found = data
        .get(repo)
        .or_else(|| {
            data.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(repo))
                .map(|(_, v)| v)
        })
        .and_then(Value::as_object);
    found.cloned().unwrap_or_default()
}

/// One action of an entry's `apply` list.
#[derive(Clone, Debug, PartialEq)]
pub struct Apply {
    /// The files that call for it, as globs: `*` within a folder, `**` any
    /// number of them, `?` one character.
    pub paths: Vec<String>,
    pub label: String,
    /// A shell command run in the clone; empty for one only you can do.
    pub command: String,
}

/// The `apply` list of the entry for the repository at `dir`, or `None`
/// when it has none (or isn't on GitHub), and then the defaults decide.
pub fn apply_of(dir: &Path) -> Option<Vec<Apply>> {
    let repo = repo_at(dir)?;
    let entry = entry_of(&read_safely(), &repo);
    let list = entry.get("apply")?.as_array()?;
    Some(
        list.iter()
            .filter_map(Value::as_object)
            .map(|a| Apply {
                paths: a
                    .get("paths")
                    .and_then(Value::as_array)
                    .map(|p| {
                        p.iter()
                            .filter_map(Value::as_str)
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default(),
                label: a.get("label").map(util::js_string).unwrap_or_default(),
                command: a
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_entry_is_found_by_its_owner_and_name_in_any_case() {
        let data = json!({ "Acme/App": { "apply": [] }, "other/x": 1 });
        let data = data.as_object().unwrap();
        assert!(entry_of(data, "acme/app").contains_key("apply"));
        assert!(entry_of(data, "Acme/App").contains_key("apply"));
        assert!(entry_of(data, "other/x").is_empty(), "not an object");
        assert!(entry_of(data, "none/here").is_empty());
        assert_eq!(
            repo_of_url("git@github.com:Acme/App.git").as_deref(),
            Some("Acme/App")
        );
        assert_eq!(repo_of_url("https://gitlab.com/a/b"), None);
    }
}
