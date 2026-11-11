//! Your settings for each repository, in `~/.config/switchboard/repos.json`,
//! by its origin's `owner/name` on GitHub:
//!
//! ```json
//! { "owner/name": { "landing": "direct", "test": "npm test", "squash": true,
//!                   "testIn": "here", "review": "review-loop", "beforePush": "",
//!                   "afterRebase": "cargo clippy --all-targets",
//!                   "apply": [{ "paths": ["db/**"], "label": "Reload the dev server", "command": "make reload" }] } }
//! ```
//!
//! `landing` is how its work lands: `pr`, through a pull request, or
//! `direct`, straight on its default branch. The rest of the landing keys
//! are how a direct landing goes: `test` the command run before the review
//! and again after the last rebase (empty, none; not set, what the
//! repository's CLAUDE.md names), `testIn` where they run, `here` (in the
//! worktree) or `ci` (the repository's CI, on a branch of the landing's own
//! pushed for it), `squash` whether the branch lands as one commit, `review` `review-loop` or `none`, `beforePush` a command
//! that must pass before the push (empty, none), and `afterRebase` a command
//! run after the last rebase in place of the tests, when that rebase had no
//! conflict and the default branch's CI isn't failing (empty or not set, the
//! tests run again), since a clean rebase changes nothing the tests haven't
//! seen pass on each side. `apply` is what a change
//! landed there needs to take effect, which `switchboard apply-actions`
//! lists in place of what it would work out by itself. The file is yours
//! and never in this repository; Settings changes it (`set_repo`), never
//! over a file it can't read.
//!
//! With no `landing` for a repository, the default decides (`landing_for`):
//! a pull request when GitHub says the default branch needs one (a
//! ruleset, branch protection, or a merge queue), else straight on it when
//! the repository's CLAUDE.md says its work goes there with no PR, as a
//! headless Haiku reads that file alone, else a pull request. The answer is
//! kept a day in the state directory (`repos-landing.json`), so nothing
//! waits on GitHub or Claude twice; off the network, or with the github
//! setting off, the last answer stands, and with none it is a pull
//! request, the way that can't push to the default branch by mistake.
//! `switchboard landing` prints it, and queue-and-clean asks it which way
//! to land. Ported from lib/repos.js and bin/apply-actions' `entry`.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::integrations::github;
use crate::{paths, requests, settings, util, worktree};

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
    read_file(&file())
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

// ── How a repository lands ──────────────────────────────────────────────────

/// How work can land.
pub const METHODS: [&str; 2] = ["pr", "direct"];
/// The reviews a direct landing can have.
pub const REVIEWS: [&str; 2] = ["review-loop", "none"];
/// Where a direct landing's tests can run: in the worktree, or in the
/// repository's CI.
pub const TEST_IN: [&str; 2] = ["here", "ci"];
/// An answer worked out is kept this long, in ms.
pub const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// A direct landing's steps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Steps {
    /// The tests' command; `None` when not set, so a landing runs what the
    /// repository's CLAUDE.md names, where an empty one is none.
    pub test: Option<String>,
    /// Where the tests run, one of `TEST_IN`.
    pub test_in: String,
    pub squash: bool,
    pub review: String,
    pub before_push: String,
    /// Run after a clean rebase in place of the tests; empty, the tests.
    pub after_rebase: String,
}

impl Default for Steps {
    fn default() -> Steps {
        Steps {
            test: None,
            test_in: "here".into(),
            squash: true,
            review: "review-loop".into(),
            before_push: String::new(),
            after_rebase: String::new(),
        }
    }
}

impl Steps {
    /// As JSON, with the file's names.
    pub fn json(&self) -> Value {
        json!({
            "test": self.test,
            "testIn": self.test_in,
            "squash": self.squash,
            "review": self.review,
            "beforePush": self.before_push,
            "afterRebase": self.after_rebase,
        })
    }
}

/// Whether a landing key holds a value it can.
fn valid(key: &str, value: &Value) -> bool {
    match key {
        "landing" => value.as_str().is_some_and(|v| METHODS.contains(&v)),
        "review" => value.as_str().is_some_and(|v| REVIEWS.contains(&v)),
        "testIn" => value.as_str().is_some_and(|v| TEST_IN.contains(&v)),
        "test" | "beforePush" | "afterRebase" => value.is_string(),
        "squash" => value.is_boolean(),
        _ => false,
    }
}

/// A repository's landing keys, only those holding a value they can.
pub fn landing_entry(data: &Map<String, Value>, repo: &str) -> Map<String, Value> {
    entry_of(data, repo)
        .into_iter()
        .filter(|(k, v)| valid(k, v))
        .collect()
}

/// A direct landing's steps from an entry's keys, the defaults for the rest.
pub fn steps_of(entry: &Map<String, Value>) -> Steps {
    let text = |k: &str| entry.get(k).and_then(Value::as_str).map(str::to_string);
    let d = Steps::default();
    Steps {
        test: text("test"),
        test_in: text("testIn").unwrap_or(d.test_in),
        squash: entry
            .get("squash")
            .and_then(Value::as_bool)
            .unwrap_or(d.squash),
        review: text("review").unwrap_or(d.review),
        before_push: text("beforePush").unwrap_or(d.before_push),
        after_rebase: text("afterRebase").unwrap_or(d.after_rebase),
    }
}

/// Sets one landing key of a repository's entry, keeping the rest of it;
/// `landing` `automatic` takes the key out, so the default decides, and an
/// entry left empty goes. Never over a file that can't be read as a JSON
/// object, the version before kept as `repos.json.bak`.
pub fn set_repo(repo: &str, key: &str, value: Value) -> Result<(), String> {
    set_repo_in(&file(), repo, key, value)
}

/// `set_repo` on the file at `path`.
pub fn set_repo_in(path: &Path, repo: &str, key: &str, value: Value) -> Result<(), String> {
    let automatic = key == "landing" && value.as_str() == Some("automatic");
    if !automatic && !valid(key, &value) {
        return Err(
            if [
                "landing",
                "test",
                "testIn",
                "squash",
                "review",
                "beforePush",
                "afterRebase",
            ]
            .contains(&key)
            {
                format!("not a value for {key}: {value}")
            } else {
                format!("not a landing setting: {key}")
            },
        );
    }
    let mut data = read_file(path).map_err(|e| {
        format!(
            "{} can't be read ({e}); fix it by hand first",
            paths::tilde(path)
        )
    })?;
    // The entry under the name the file has it by, in whatever case.
    let name = data
        .keys()
        .find(|k| k.eq_ignore_ascii_case(repo))
        .cloned()
        .unwrap_or_else(|| repo.to_string());
    let mut entry = data
        .get(&name)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if automatic {
        entry.remove(key);
    } else {
        entry.insert(key.to_string(), value);
    }
    if entry.is_empty() {
        data.remove(&name);
    } else {
        data.insert(name, Value::Object(entry));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    if path.exists() {
        let mut bak = path.as_os_str().to_owned();
        bak.push(".bak");
        std::fs::copy(path, bak).map_err(|e| e.to_string())?;
    }
    util::write_atomic(
        path,
        format!("{}\n", util::json_pretty(&Value::Object(data))),
    )
    .map_err(|e| e.to_string())
}

/// `repos-landing.json` in the state directory: each repository's landing as
/// last worked out, `{ landing, why, at }`.
pub fn cache_file() -> PathBuf {
    paths::state_dir().join("repos-landing.json")
}

/// An answer worked out before, however old.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cached {
    pub landing: String,
    pub why: String,
    pub at: i64,
}

/// The answer last worked out for a repository, by its key.
pub fn cached_in(file: &Path, key: &str) -> Option<Cached> {
    let data = util::read_json(file)?;
    let answer = data.get(key)?;
    let landing = answer.get("landing")?.as_str()?;
    METHODS.contains(&landing).then(|| Cached {
        landing: landing.to_string(),
        why: answer
            .get("why")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        at: answer.get("at").and_then(Value::as_f64).unwrap_or(0.0) as i64,
    })
}

fn remember(file: &Path, key: &str, answer: &Cached) {
    let mut data = util::read_json_object(file);
    data.insert(
        key.to_string(),
        json!({ "landing": answer.landing, "why": answer.why, "at": util::js_number(answer.at as f64) }),
    );
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = util::write_atomic(file, util::json_compact(&Value::Object(data)));
}

/// How a repository lands, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Landing {
    /// `owner/name` on GitHub, when its origin is there.
    pub repo: Option<String>,
    /// What it is known by here: `owner/name`, else its git directory.
    pub key: String,
    /// `pr` or `direct`.
    pub landing: String,
    pub why: String,
    /// Where the method came from: `repos.json`, `github`, `claude-md`,
    /// `default` (neither said), `cached` (a day old at most) or `offline`
    /// (GitHub or Claude couldn't be asked: the last answer, else a PR).
    pub source: String,
    pub steps: Steps,
}

/// What GitHub says of a repository's default branch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rules {
    pub branch: Option<String>,
    pub requires_pr: bool,
    pub why: String,
}

/// How the default is worked out: GitHub asked of a repository's rules,
/// and Claude of its CLAUDE.md (`direct` or `pr`); the tests give their own.
pub struct Ask<'a> {
    pub github: &'a dyn Fn(&str) -> Result<Rules, String>,
    pub claude: &'a dyn Fn(&str) -> Result<String, String>,
}

/// Where the answers are: the file of yours and the day's cache.
pub struct Files {
    pub repos: PathBuf,
    pub cache: PathBuf,
}

/// The repository at `dir`: its `owner/name` on GitHub, its worktree's top,
/// and what it is known by here.
fn repository_at(dir: &Path) -> Option<(Option<String>, PathBuf, String)> {
    let top = PathBuf::from(worktree::git_at(dir, &["rev-parse", "--show-toplevel"]).ok()?);
    let repo = repo_at(dir);
    let common = crate::cmd::land_lock::common_dir(dir);
    let key = repo.clone().unwrap_or_else(|| {
        common
            .as_deref()
            .unwrap_or(&top)
            .to_string_lossy()
            .into_owned()
    });
    Some((repo, top, key))
}

/// How the repository at `dir` lands as far as is known without asking
/// anyone: what repos.json sets, else the last answer worked out, of any
/// age; `None` when neither says. It runs git, so off the server's task.
pub fn landing_known(dir: &Path) -> Option<String> {
    let (_, _, key) = repository_at(dir)?;
    let set = landing_entry(&read_safely(), &key)
        .get("landing")
        .and_then(Value::as_str)
        .map(str::to_string);
    set.or_else(|| cached_in(&cache_file(), &key).map(|c| c.landing))
}

/// How the repository at `dir` lands, as the module's comment says.
pub fn landing_for(dir: &Path, refresh: bool) -> Result<Landing, String> {
    let ask = Ask {
        github: &ask_github,
        claude: &ask_claude_md,
    };
    let files = Files {
        repos: file(),
        cache: cache_file(),
    };
    landing_with(
        dir,
        &ask,
        &files,
        util::now_ms(),
        refresh,
        settings::is_on("github"),
    )
}

/// `landing_for` with what it asks and reads given.
pub fn landing_with(
    dir: &Path,
    ask: &Ask,
    files: &Files,
    now: i64,
    refresh: bool,
    github_on: bool,
) -> Result<Landing, String> {
    let (repo, top, key) =
        repository_at(dir).ok_or_else(|| format!("{} isn't in a git repository", dir.display()))?;
    // A repos.json that can't be read stops it, rather than landing without
    // what it may say.
    let data = read_file(&files.repos).map_err(|e| {
        format!(
            "{} can't be read ({e}); fix it first",
            paths::tilde(&files.repos)
        )
    })?;
    let entry = landing_entry(&data, &key);
    let steps = steps_of(&entry);
    let answer = |landing: &str, why: String, source: &str| Landing {
        repo: repo.clone(),
        key: key.clone(),
        landing: landing.to_string(),
        why,
        source: source.to_string(),
        steps: steps.clone(),
    };
    if let Some(method) = entry.get("landing").and_then(Value::as_str) {
        return Ok(answer(method, "set in repos.json".into(), "repos.json"));
    }
    let cached = cached_in(&files.cache, &key);
    if let Some(c) = cached.as_ref().filter(|c| !refresh && now - c.at < DAY_MS) {
        return Ok(answer(&c.landing, c.why.clone(), "cached"));
    }
    // The github setting off asks GitHub nothing: the last answer stands,
    // else a pull request.
    if !github_on {
        return Ok(match cached {
            Some(c) => answer(
                &c.landing,
                format!("{} (as last found; the github setting is off)", c.why),
                "offline",
            ),
            None => answer(
                "pr",
                "the github setting is off, so through a PR unless repos.json says otherwise"
                    .into(),
                "offline",
            ),
        });
    }
    let found = (|| -> Result<(String, String, String), String> {
        let rules = match &repo {
            Some(repo) => (ask.github)(repo)?,
            None => Rules {
                branch: None,
                requires_pr: false,
                why: String::new(),
            },
        };
        if rules.requires_pr {
            return Ok(("pr".into(), rules.why, "github".into()));
        }
        // Only a file inside the repository, a link out of it never
        // followed, since what is read is sent to Claude.
        let text = std::fs::canonicalize(top.join("CLAUDE.md"))
            .ok()
            .filter(|f| std::fs::canonicalize(&top).is_ok_and(|t| f.starts_with(&t) && *f != t))
            .and_then(|f| std::fs::read_to_string(f).ok())
            .filter(|t| !t.trim().is_empty());
        let said = match &text {
            Some(text) => Some((ask.claude)(text)?),
            None => None,
        };
        Ok(if said.as_deref() == Some("direct") {
            (
                "direct".into(),
                format!(
                    "its CLAUDE.md commits straight to {}",
                    rules.branch.as_deref().unwrap_or("the default branch")
                ),
                "claude-md".into(),
            )
        } else {
            (
                "pr".into(),
                if text.is_some() {
                    "its CLAUDE.md doesn't say to commit straight to the default branch".into()
                } else {
                    "nothing says otherwise".into()
                },
                "default".into(),
            )
        })
    })();
    match found {
        Ok((landing, why, source)) => {
            remember(
                &files.cache,
                &key,
                &Cached {
                    landing: landing.clone(),
                    why: why.clone(),
                    at: now,
                },
            );
            Ok(answer(&landing, why, &source))
        }
        Err(e) => Ok(match cached {
            Some(c) => answer(
                &c.landing,
                format!("{} (as last found; couldn't ask again: {e})", c.why),
                "offline",
            ),
            None => answer(
                "pr",
                format!("couldn't find out ({e}), so through a PR"),
                "offline",
            ),
        }),
    }
}

/// A file read as `read` reads repos.json.
fn read_file(path: &Path) -> Result<Map<String, Value>, String> {
    let Ok(text) = std::fs::read_to_string(path) else {
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

// Runs a request to its end from wherever this is: a blocking thread of
// the server's runtime, or a command with none.
fn wait_for<T>(future: impl std::future::Future<Output = T>) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => handle.block_on(future),
        Err(_) => util::block_on(future),
    }
}

/// Whether GitHub makes the default branch take a pull request: a merge
/// queue, branch protection asking for one (a review count at all, 0
/// included, or no push for you), or a ruleset with a pull request or merge
/// queue rule. A repository on a plan without rulesets answers 404, which
/// is none; any other failure is a failure to find out, never read as none.
pub fn ask_github(repo: &str) -> Result<Rules, String> {
    let (owner, name) = repo.split_once('/').ok_or("not owner/name")?;
    let data = wait_for(github::graphql(
        "query($o: String!, $n: String!) { repository(owner: $o, name: $n) { mergeQueue { url } defaultBranchRef { name refUpdateRule { viewerCanPush requiredApprovingReviewCount } } } }",
        json!({ "o": owner, "n": name }),
        "landing: how a repository lands",
    ))
    .map_err(|e| e.to_string())?;
    rules_of(&data, &|branch: &str| {
        let path = format!(
            "repos/{owner}/{name}/rules/branches/{}",
            branch.replace('/', "%2F")
        );
        match wait_for(github::rest(
            "GET",
            &path,
            None,
            "landing: how a repository lands",
        )) {
            Ok(v) => Ok(v),
            Err(e)
                if e.downcast_ref::<crate::net::ServiceError>()
                    .is_some_and(|s| s.kind == crate::net::ErrorKind::NotFound) =>
            {
                Ok(Value::Array(Vec::new()))
            }
            Err(e) => Err(e.to_string()),
        }
    })
}

/// GitHub's answer for a repository read as `ask_github` reads it, its
/// branch's rulesets asked of `rules`.
pub fn rules_of(
    data: &Value,
    rules: &dyn Fn(&str) -> Result<Value, String>,
) -> Result<Rules, String> {
    let repository = data
        .get("repository")
        .filter(|r| r.is_object())
        .ok_or("GitHub didn't answer")?;
    let branch = repository
        .pointer("/defaultBranchRef/name")
        .and_then(Value::as_str)
        .map(str::to_string);
    let named = branch.clone().unwrap_or_default();
    if repository.get("mergeQueue").is_some_and(Value::is_object) {
        return Ok(Rules {
            branch,
            requires_pr: true,
            why: format!("{named} has a merge queue"),
        });
    }
    let rule = repository.pointer("/defaultBranchRef/refUpdateRule");
    let protected = rule.is_some_and(|r| {
        r.get("viewerCanPush") == Some(&Value::Bool(false))
            || r.get("requiredApprovingReviewCount")
                .is_some_and(Value::is_number)
    });
    if protected {
        return Ok(Rules {
            branch,
            requires_pr: true,
            why: format!("GitHub requires a PR on {named}"),
        });
    }
    if let Some(b) = branch.as_deref() {
        let list = rules(b)?;
        let needs = list.as_array().into_iter().flatten().any(|r| {
            matches!(
                r.get("type").and_then(Value::as_str),
                Some("pull_request" | "merge_queue")
            )
        });
        if needs {
            return Ok(Rules {
                branch,
                requires_pr: true,
                why: format!("GitHub requires a PR on {named}"),
            });
        }
    }
    Ok(Rules {
        branch,
        requires_pr: false,
        why: String::new(),
    })
}

/// Whether a CLAUDE.md says the work goes straight to the default branch
/// with no PR, as a headless Haiku reads that file alone: `direct` or
/// `pr`.
pub fn ask_claude_md(text: &str) -> Result<String, String> {
    let claude = crate::diff::tell::claude_command().ok_or("claude isn't installed")?;
    requests::count("Anthropic", "claude -p", "landing: CLAUDE.md");
    let mut cmd = std::process::Command::new(claude);
    cmd.args([
        "-p",
        "--model",
        "haiku",
        "--no-session-persistence",
        "--tools",
        "",
        "--strict-mcp-config",
        "--disable-slash-commands",
        "--output-format",
        "text",
    ]);
    crate::diff::tell::headless(&mut cmd);
    let prompt = format!(
        "Below is a repository's CLAUDE.md. Does it say that work in this repository is committed straight to its default branch, with no pull request? Answer with the one word \"direct\" if it does, else \"pr\".\n\n<claude-md>\n{text}\n</claude-md>"
    );
    let out = crate::diff::tell::run_with_timeout(cmd, prompt.as_bytes(), 120)
        .map_err(|e| e.to_string())?;
    said(&out).ok_or_else(|| format!("Haiku didn't say direct or pr: {}", out.trim()))
}

/// Haiku's word: `direct` when its answer starts with it, `pr` when it
/// said either word elsewhere, nothing when it said neither.
fn said(out: &str) -> Option<String> {
    let lower = out.to_lowercase();
    if lower
        .trim_start_matches(|c: char| !c.is_alphanumeric())
        .starts_with("direct")
    {
        return Some("direct".into());
    }
    regex::Regex::new(r"\b(direct|pr)\b")
        .ok()?
        .is_match(&lower)
        .then(|| "pr".into())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    use crate::tabs::git::tests::{Scratch, git, repo};

    fn never_github(_: &str) -> Result<Rules, String> {
        panic!("GitHub isn't to be asked")
    }
    fn never_claude(_: &str) -> Result<String, String> {
        panic!("Claude isn't to be asked")
    }

    struct Rig {
        scratch: Scratch,
        files: Files,
        direct: PathBuf,
        guarded: PathBuf,
    }

    fn rig(name: &str) -> Option<Rig> {
        let scratch = Scratch::new(name);
        let direct = scratch.0.join("direct");
        let guarded = scratch.0.join("guarded");
        if !repo(&direct) || !repo(&guarded) {
            eprintln!("skipped: no git");
            return None;
        }
        git(
            &direct,
            &["remote", "add", "origin", "git@github.com:acme/direct.git"],
        );
        git(
            &guarded,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/acme/guarded.git",
            ],
        );
        let files = Files {
            repos: scratch.0.join("repos.json"),
            cache: scratch.0.join("state/repos-landing.json"),
        };
        Some(Rig {
            scratch,
            files,
            direct,
            guarded,
        })
    }

    #[test]
    fn a_repositorys_entry_decides_with_its_steps_asking_neither_github_nor_claude() {
        let Some(r) = rig("repos-entry") else {
            return;
        };
        let never = Ask {
            github: &never_github,
            claude: &never_claude,
        };
        std::fs::write(
            &r.files.repos,
            r#"{"acme/direct": {"landing": "direct", "test": "npm test", "squash": false}}"#,
        )
        .unwrap();
        let answer = landing_with(&r.direct, &never, &r.files, 1, false, true).unwrap();
        assert_eq!(
            (answer.landing.as_str(), answer.source.as_str()),
            ("direct", "repos.json")
        );
        assert_eq!(
            answer.steps,
            Steps {
                test: Some("npm test".into()),
                test_in: "here".into(),
                squash: false,
                review: "review-loop".into(),
                before_push: String::new(),
                after_rebase: String::new(),
            },
            "what the entry leaves out takes the defaults"
        );
        std::fs::write(&r.files.repos, r#"{"acme/direct": {"landing": "direct"}}"#).unwrap();
        let answer = landing_with(&r.direct, &never, &r.files, 1, false, true).unwrap();
        assert_eq!(
            answer.steps.test, None,
            "a test command not set is none, for the CLAUDE.md's tests"
        );
        std::fs::write(
            &r.files.repos,
            r#"{"acme/direct": {"landing": "direct", "testIn": "ci"}}"#,
        )
        .unwrap();
        let answer = landing_with(&r.direct, &never, &r.files, 1, false, true).unwrap();
        assert_eq!(answer.steps.test_in, "ci");
        assert_eq!(answer.steps.json()["testIn"], "ci");
        std::fs::write(
            &r.files.repos,
            r#"{"acme/direct": {"landing": "direct", "testIn": "elsewhere"}}"#,
        )
        .unwrap();
        let answer = landing_with(&r.direct, &never, &r.files, 1, false, true).unwrap();
        assert_eq!(
            answer.steps.test_in, "here",
            "a place the tests can't run is left out, so they run in the worktree"
        );
        std::fs::write(&r.files.repos, "{ broken").unwrap();
        let err = landing_with(&r.direct, &never, &r.files, 1, false, true).unwrap_err();
        assert!(err.contains("can't be read"), "{err}");
    }

    #[test]
    fn a_claude_md_committing_straight_to_the_branch_makes_it_direct_kept_a_day_and_github_requiring_a_pr_wins()
     {
        let Some(r) = rig("repos-default") else {
            return;
        };
        std::fs::write(
            r.direct.join("CLAUDE.md"),
            "Commit straight to main: there is no PR flow here.\n",
        )
        .unwrap();
        let read = std::cell::RefCell::new(String::new());
        let open = |_: &str| {
            Ok(Rules {
                branch: Some("main".into()),
                requires_pr: false,
                why: String::new(),
            })
        };
        let claude = |text: &str| {
            *read.borrow_mut() = text.to_string();
            Ok("direct".to_string())
        };
        let ask = Ask {
            github: &open,
            claude: &claude,
        };
        let now = 1_700_000_000_000;
        let answer = landing_with(&r.direct, &ask, &r.files, now, false, true).unwrap();
        assert_eq!(
            (answer.landing.as_str(), answer.source.as_str()),
            ("direct", "claude-md")
        );
        assert_eq!(answer.why, "its CLAUDE.md commits straight to main");
        assert!(
            read.borrow().contains("no PR flow"),
            "Claude is given the CLAUDE.md"
        );
        let never = Ask {
            github: &never_github,
            claude: &never_claude,
        };
        let again = landing_with(&r.direct, &never, &r.files, now + 1000, false, true).unwrap();
        assert_eq!(
            (again.landing.as_str(), again.source.as_str()),
            ("direct", "cached")
        );

        let queue = |_: &str| {
            Ok(Rules {
                branch: Some("main".into()),
                requires_pr: true,
                why: "main has a merge queue".into(),
            })
        };
        let ask = Ask {
            github: &queue,
            claude: &never_claude,
        };
        let pr = landing_with(&r.guarded, &ask, &r.files, now, false, true).unwrap();
        assert_eq!(
            (pr.landing.as_str(), pr.source.as_str(), pr.why.as_str()),
            ("pr", "github", "main has a merge queue")
        );
    }

    #[test]
    fn off_the_network_the_last_answer_stands_and_with_none_or_github_off_it_is_a_pr() {
        let Some(r) = rig("repos-offline") else {
            return;
        };
        let down = |_: &str| -> Result<Rules, String> { Err("no network".into()) };
        let offline = Ask {
            github: &down,
            claude: &never_claude,
        };
        let now = 1_700_000_000_000;
        let none = landing_with(&r.guarded, &offline, &r.files, now, false, true).unwrap();
        assert_eq!(
            (none.landing.as_str(), none.source.as_str()),
            ("pr", "offline")
        );

        std::fs::write(r.direct.join("CLAUDE.md"), "Commit straight to main.\n").unwrap();
        let open = |_: &str| {
            Ok(Rules {
                branch: Some("main".into()),
                requires_pr: false,
                why: String::new(),
            })
        };
        let direct = |_: &str| Ok("direct".to_string());
        let ask = Ask {
            github: &open,
            claude: &direct,
        };
        landing_with(&r.direct, &ask, &r.files, now, false, true).unwrap();
        let later =
            landing_with(&r.direct, &offline, &r.files, now + 2 * DAY_MS, false, true).unwrap();
        assert_eq!(
            (later.landing.as_str(), later.source.as_str()),
            ("direct", "offline"),
            "the last answer stands, however old"
        );

        let never = Ask {
            github: &never_github,
            claude: &never_claude,
        };
        let off = landing_with(&r.guarded, &never, &r.files, now, false, false).unwrap();
        assert_eq!(off.landing, "pr");
        assert!(off.why.contains("github setting is off"), "{}", off.why);
    }

    #[test]
    fn a_claude_md_linking_out_of_its_repository_is_never_read_or_sent() {
        let Some(r) = rig("repos-link") else {
            return;
        };
        let secret = r.scratch.0.join("secret");
        std::fs::write(&secret, "a key\n").unwrap();
        std::os::unix::fs::symlink(&secret, r.direct.join("CLAUDE.md")).unwrap();
        let open = |_: &str| {
            Ok(Rules {
                branch: Some("main".into()),
                requires_pr: false,
                why: String::new(),
            })
        };
        let ask = Ask {
            github: &open,
            claude: &never_claude,
        };
        let answer = landing_with(&r.direct, &ask, &r.files, 1, false, true).unwrap();
        assert_eq!(
            (answer.landing.as_str(), answer.why.as_str()),
            ("pr", "nothing says otherwise")
        );
    }

    #[test]
    fn set_repo_keeps_the_rest_of_an_entry_and_never_writes_over_a_file_it_cant_read() {
        let scratch = Scratch::new("repos-set");
        let path = scratch.0.join("repos.json");
        set_repo_in(&path, "acme/direct", "landing", json!("direct")).unwrap();
        set_repo_in(&path, "acme/direct", "test", json!("make check")).unwrap();
        assert_eq!(
            read_file(&path).unwrap(),
            *json!({ "acme/direct": { "landing": "direct", "test": "make check" } })
                .as_object()
                .unwrap()
        );
        set_repo_in(&path, "acme/direct", "landing", json!("automatic")).unwrap();
        assert_eq!(
            read_file(&path).unwrap(),
            *json!({ "acme/direct": { "test": "make check" } })
                .as_object()
                .unwrap(),
            "the default decides again, the steps kept"
        );
        assert!(
            set_repo_in(&path, "acme/direct", "review", json!("maybe"))
                .unwrap_err()
                .contains("not a value")
        );
        assert!(
            set_repo_in(&path, "acme/direct", "testIn", json!("elsewhere"))
                .unwrap_err()
                .contains("not a value for testIn")
        );
        set_repo_in(&path, "acme/direct", "testIn", json!("ci")).unwrap();
        assert_eq!(
            read_file(&path).unwrap()["acme/direct"]["testIn"],
            json!("ci")
        );
        assert!(
            set_repo_in(&path, "acme/direct", "afterRebase", json!(true))
                .unwrap_err()
                .contains("not a value for afterRebase")
        );
        set_repo_in(&path, "acme/direct", "afterRebase", json!("make lint")).unwrap();
        let written = read_file(&path).unwrap();
        assert_eq!(written["acme/direct"]["afterRebase"], json!("make lint"));
        let steps = steps_of(&landing_entry(&written, "acme/direct"));
        assert_eq!(steps.after_rebase, "make lint");
        assert_eq!(steps.json()["afterRebase"], "make lint");
        std::fs::write(&path, "{ not json").unwrap();
        assert!(
            set_repo_in(&path, "acme/direct", "landing", json!("pr"))
                .unwrap_err()
                .contains("can't be read")
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
    }

    #[test]
    fn github_s_rules_a_queue_protection_or_a_ruleset_need_a_pr_and_a_404_is_none() {
        let answer = |queue: bool, rule: Value| {
            json!({ "repository": { "mergeQueue": if queue { json!({"url": "u"}) } else { Value::Null },
                "defaultBranchRef": { "name": "main", "refUpdateRule": rule } } })
        };
        let none = |_: &str| Ok(Value::Array(Vec::new()));
        assert!(
            rules_of(&answer(true, Value::Null), &none)
                .unwrap()
                .requires_pr
        );
        assert!(
            rules_of(
                &answer(
                    false,
                    json!({"viewerCanPush": true, "requiredApprovingReviewCount": 0})
                ),
                &none
            )
            .unwrap()
            .requires_pr,
            "protection asking for a PR with no approvals still asks for one"
        );
        assert!(
            !rules_of(&answer(false, Value::Null), &none)
                .unwrap()
                .requires_pr
        );
        let ruleset = |_: &str| Ok(json!([{ "type": "pull_request" }]));
        assert!(
            rules_of(&answer(false, Value::Null), &ruleset)
                .unwrap()
                .requires_pr
        );
        let failing = |_: &str| -> Result<Value, String> { Err("500".into()) };
        assert!(
            rules_of(&answer(false, Value::Null), &failing).is_err(),
            "a ruleset that can't be read is a failure to find out"
        );
        assert_eq!(said("direct"), Some("direct".into()));
        assert_eq!(said("**Direct**."), Some("direct".into()));
        assert_eq!(said("pr"), Some("pr".into()));
        assert_eq!(said("I can't tell"), None);
    }
}
