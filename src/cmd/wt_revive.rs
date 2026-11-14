//! `wt-revive`: the worktrees no tab has open, for mission control's Work
//! map, whose `m` menu shows only them: Enter there opens a tab in one and
//! resumes its Claude session with `claude-here`, as `wtr` does. Ported
//! from `bin/wt-revive`.
//!
//! ```text
//! wt-revive          print the rows, from the repository of the current directory
//! wt-revive <dir>    the same, starting from <dir>'s repository
//! wt-revive --json   each row as a line of JSON, nothing when there are none
//! wt-revive --sessions --json
//!                    each Claude session of the last two weeks instead, newest first
//! ```
//!
//! Which repositories: the one `<dir>` is in, every open pane's, and every
//! one a Claude session has run in over the last two weeks, so a repository
//! whose tabs are all closed still counts. Each row is a worktree of one of
//! them with no pane inside it: its repository and branch, how long since
//! it was last used, and the last prompt Claude was given there, most
//! recent first. Worktrees git marks prunable (their directory is gone) are
//! left out, and so are those of someone else's PR, checked out untrusted
//! in `~/.cache/switchboard/pr-review` (src/trust.rs), which no Claude is
//! started in.
//!
//! `--sessions` lists the conversations themselves, for mission control's
//! Reopen view: each of the last two weeks (a subagent's left out) with its
//! id, the directory it ended in, when it was last used, its last prompt and
//! how many prompts you gave it, leaving out one that ended in someone
//! else's PR's worktree. Which of them a tab still runs is the view's to
//! say.
//!
//! What a transcript says (its last directory, last prompt and prompts
//! counted) is cached in
//! `~/.cache/wt-revive/index.json` against the file's size and modified
//! time, and what repository a directory belongs to against the directory,
//! in the shape the Node version wrote. Between two looks almost no
//! transcript has changed, so a press reads only the few that have.
//!
//! The panes' directories come from the server (`switchboard list`) where
//! tmux's `list-panes` gave them. Mission control's Reopen view runs
//! `wt-revive --json ~` off the server's task, as the daemon ran
//! bin/wt-revive, keeping the rows it had when a look fails.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::{paths, util};

const RECENT_MS: i64 = 14 * 24 * 60 * 60 * 1000;
/// The directory is on most lines of a transcript, so the end of it says
/// where the session finished, and some transcripts are megabytes long.
const CWD_TAIL: u64 = 64 * 1024;
/// Bumped when what `tail_facts` reads changes, so the cache is read again.
const FACTS: i64 = 6;
/// A prompt can be followed by any amount of tool output, so the search for
/// it widens until it finds one or has read the whole file.
const PROMPT_TAILS: [u64; 3] = [256 * 1024, 2 * 1024 * 1024, u64::MAX];
/// How long a directory found to be outside any repository is believed. A
/// repository's directory stays one, but a plain directory can become one.
const NOT_A_REPO_MS: i64 = 24 * 60 * 60 * 1000;

/// A worktree with no tab, as Reopen lists it and `--json` prints it.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Row {
    /// When it was last used, in seconds since the epoch.
    pub when: i64,
    pub tree: String,
    pub repo: String,
    pub branch: String,
    /// `<repository's name> <branch>`.
    pub label: String,
    pub age: String,
    pub prompt: String,
}

fn cache_file() -> PathBuf {
    paths::home_join(".cache/wt-revive/index.json")
}

fn read_cache() -> (Map<String, Value>, Map<String, Value>) {
    let cache = util::read_json_object(cache_file());
    let part = |key: &str| {
        cache
            .get(key)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default()
    };
    (part("transcripts"), part("repos"))
}

/// The cache holds prompts, so only you can read it; written to a
/// temporary file and renamed, so two presses at once never leave half a
/// file behind.
fn write_cache(transcripts: Map<String, Value>, repos: Map<String, Value>) {
    let file = cache_file();
    if let Some(dir) = file.parent() {
        use std::os::unix::fs::DirBuilderExt;
        let _ = fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir);
    }
    let text = util::json_compact(&json!({ "transcripts": transcripts, "repos": repos }));
    let tmp = file.with_file_name(format!("index.json.{}", std::process::id()));
    if util::write_private(&tmp, text, false).is_ok() && fs::rename(&tmp, &file).is_err() {
        let _ = fs::remove_file(&tmp);
    }
}

/// The last `length` bytes of a file, from the start of a whole line unless
/// the read reaches the start of the file; and whether it did.
fn tail_of(path: &Path, size: u64, length: u64) -> Option<(String, bool)> {
    let mut file = File::open(path).ok()?;
    let start = size.saturating_sub(length);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    file.take(size - start).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    if start == 0 {
        return Some((text, true));
    }
    let from = text.find('\n').map_or(text.len(), |at| at + 1);
    Some((text[from..].to_string(), false))
}

fn parse_time(text: &str) -> Option<f64> {
    text.parse::<jiff::Timestamp>()
        .ok()
        .map(|t| t.as_millisecond() as f64)
}

/// The last directory and the last time a transcript was used. The last
/// directory counts, since a session that moved into a worktree belongs to
/// the worktree; the last timestamp, not the file's modified time, since
/// Claude Code appends entries with none whenever a session is merely
/// opened. Both are whole lines' own, read from the end, never a "cwd" or
/// "timestamp" inside a tool call's input; the piece read grows until it
/// holds them or is the whole file.
fn tail_facts(path: &Path, size: u64, mtime_ms: f64) -> (Option<String>, f64) {
    let mut length = CWD_TAIL;
    loop {
        let Some((text, whole)) = tail_of(path, size, length) else {
            return (None, mtime_ms);
        };
        let mut cwd = None;
        let mut active = None;
        for line in text.split('\n').rev() {
            if cwd.is_some() && active.is_some() {
                break;
            }
            let Ok(entry) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if cwd.is_none()
                && let Some(c) = entry
                    .get("cwd")
                    .and_then(Value::as_str)
                    .filter(|c| !c.is_empty())
            {
                cwd = Some(c.to_string());
            }
            if active.is_none() {
                active = entry
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .and_then(parse_time);
            }
        }
        if (cwd.is_some() && active.is_some()) || whole {
            return (cwd, active.unwrap_or(mtime_ms));
        }
        length = length.saturating_mul(4);
    }
}

/// What you typed, not what Claude Code added: string content or text
/// blocks (`typed_text`), nothing marked meta, and nothing that starts with a
/// tag (slash commands, hook output).
fn prompts_in(text: &str) -> Vec<String> {
    text.split('\n')
        .filter(|line| line.contains(r#""type":"user""#))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|e| {
            e.get("type").and_then(Value::as_str) == Some("user")
                && !e.get("isMeta").is_some_and(util::truthy)
        })
        .filter_map(|e| typed_text(e.pointer("/message/content")?))
        .map(|c| c.replace(['\n', '\t'], " "))
        .filter(|p| !p.starts_with('<'))
        .collect()
}

/// What a prompt's content says: a string, or the text blocks of a list
/// (a prompt with a picture), never a list holding a tool's result.
fn typed_text(content: &Value) -> Option<String> {
    if let Some(text) = content.as_str() {
        return Some(text.to_string());
    }
    let parts = content.as_array()?;
    if parts
        .iter()
        .any(|p| p.get("type").and_then(Value::as_str) == Some("tool_result"))
    {
        return None;
    }
    let text: Vec<&str> = parts
        .iter()
        .filter(|p| p.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|p| p.get("text").and_then(Value::as_str))
        .collect();
    (!text.is_empty()).then(|| text.join(" "))
}

/// The last prompt that says something: "yes" and "go ahead" don't say what
/// the worktree was for. The very last one if none does.
fn prompt_in(path: &Path, size: u64) -> String {
    let mut answer = None;
    for length in PROMPT_TAILS {
        let Some((text, whole)) = tail_of(path, size, length) else {
            break;
        };
        let prompts = prompts_in(&text);
        answer = prompts
            .iter()
            .rev()
            .find(|p| p.encode_utf16().count() >= 20)
            .cloned();
        if answer.is_none() && whole {
            answer = Some(prompts.last().cloned().unwrap_or_default());
        }
        if answer.is_some() || whole {
            break;
        }
    }
    answer.unwrap_or_default().chars().take(90).collect()
}

/// A transcript of the last two weeks and what it says.
struct Recent {
    file: String,
    entry: Map<String, Value>,
}

impl Recent {
    fn cwd(&self) -> &str {
        self.entry.get("cwd").and_then(Value::as_str).unwrap_or("")
    }
    fn active_ms(&self) -> f64 {
        self.entry
            .get("activeMs")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
    }
    fn size(&self) -> u64 {
        self.entry.get("size").and_then(Value::as_u64).unwrap_or(0)
    }
}

fn mtime_ms(meta: &fs::Metadata) -> f64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

fn jsonl_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_dir() => jsonl_files(&path, out),
            _ if path.extension().is_some_and(|x| x == "jsonl") => out.push(path),
            _ => {}
        }
    }
}

/// Every transcript modified in the last two weeks, subagents' included,
/// last used first, each with the directory it ended in. Only a file whose
/// size or modified time differs from the cache's is read.
fn recent_sessions(cache: &Map<String, Value>) -> Vec<Recent> {
    let mut files = Vec::new();
    jsonl_files(&paths::claude_dir().join("projects"), &mut files);
    let since = (util::now_ms() - RECENT_MS) as f64;
    let mut found: Vec<Recent> = files
        .into_iter()
        .filter_map(|path| {
            let meta = fs::metadata(&path).ok()?;
            let mtime = mtime_ms(&meta);
            if mtime <= since {
                return None;
            }
            let size = meta.len();
            let file = path.to_string_lossy().into_owned();
            let known = cache.get(&file).and_then(Value::as_object);
            let fresh = known.filter(|k| {
                k.get("mtimeMs").and_then(Value::as_f64) == Some(mtime)
                    && k.get("size").and_then(Value::as_u64) == Some(size)
                    && k.get("facts").and_then(Value::as_i64) == Some(FACTS)
            });
            let entry = match fresh {
                Some(k) => k.clone(),
                None => {
                    let (cwd, active) = tail_facts(&path, size, mtime);
                    let mut e = Map::new();
                    e.insert("mtimeMs".into(), util::js_number(mtime));
                    e.insert("size".into(), json!(size));
                    e.insert("facts".into(), json!(FACTS));
                    if let Some(cwd) = cwd {
                        e.insert("cwd".into(), json!(cwd));
                    }
                    e.insert("activeMs".into(), util::js_number(active));
                    e
                }
            };
            Some(Recent { file, entry })
        })
        .filter(|r| !r.cwd().is_empty())
        .collect();
    found.sort_by(|a, b| b.active_ms().total_cmp(&a.active_ms()));
    found
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("--no-optional-locks")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// A directory's repository, as its main checkout, so each one's worktrees
/// are listed once however many places point into it. A bare repository
/// has no checkout of its own.
fn repo_of(dir: &str, repos: &mut Map<String, Value>) -> Option<String> {
    if let Some(known) = repos.get(dir) {
        let repo = known.get("repo").and_then(Value::as_str);
        let checked = known
            .get("checkedAt")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        if repo.is_some() || (util::now_ms() as f64) - checked < NOT_A_REPO_MS as f64 {
            return repo.map(str::to_string);
        }
    }
    // Made absolute here rather than with `--path-format=absolute`, which
    // git before 2.31 doesn't have.
    let common = git(Path::new(dir), &["rev-parse", "--git-common-dir"]);
    let repo = common.and_then(|c| {
        let p = crate::worktree::normalize(&Path::new(dir).join(c));
        (p.file_name().is_some_and(|n| n == ".git"))
            .then(|| p.parent().map(|d| d.to_string_lossy().into_owned()))
            .flatten()
    });
    repos.insert(
        dir.to_string(),
        json!({ "repo": repo, "checkedAt": util::now_ms() }),
    );
    repo
}

fn worktrees_of(repo: &str) -> Vec<(String, String)> {
    let porcelain = git(Path::new(repo), &["worktree", "list", "--porcelain"]).unwrap_or_default();
    crate::worktree::parse_worktrees(&porcelain)
        .into_iter()
        .filter(|w| !w.prunable && !w.bare)
        // Never someone else's PR, which no Claude is started in.
        .filter(|w| !crate::trust::in_review_dir(&w.path))
        .map(|w| {
            let branch = if w.detached {
                "(detached)".to_string()
            } else {
                w.branch.unwrap_or_default()
            };
            (w.path.to_string_lossy().into_owned(), branch)
        })
        .collect()
}

/// A Claude session, as `--sessions --json` prints it.
#[derive(Clone, Debug, Serialize, serde::Deserialize, PartialEq, Eq, Default)]
pub struct SessionRow {
    pub id: String,
    pub dir: String,
    /// When it was last used, in seconds since the epoch.
    pub when: i64,
    pub prompt: String,
    /// The prompts you gave it.
    pub turns: usize,
    /// Where `claude --resume` finds it: the folder its transcript's
    /// project is named from (`claude_here::resume_dir`); a session with
    /// none there any more isn't listed.
    #[serde(default)]
    pub resume: String,
}

/// The prompts you gave a transcript, a line at a time: what `prompts_in`
/// finds, and the slash commands you typed (`<command-name>`), which it
/// passes over for the last prompt.
fn turns_in(path: &Path) -> usize {
    use std::io::BufRead;
    let Ok(file) = File::open(path) else {
        return 0;
    };
    std::io::BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter(|l| l.contains(r#""type":"user""#))
        .filter(|l| !prompts_in(l).is_empty() || typed_command(l))
        .count()
}

/// Whether a transcript's line is a slash command you typed, its
/// `<command-name>` first or after its `<command-message>`.
fn typed_command(line: &str) -> bool {
    let Ok(e) = serde_json::from_str::<Value>(line) else {
        return false;
    };
    e.get("type").and_then(Value::as_str) == Some("user")
        && !e.get("isMeta").is_some_and(util::truthy)
        && e.pointer("/message/content")
            .and_then(typed_text)
            .is_some_and(|t| t.contains("<command-name>"))
}

/// Each conversation of the last two weeks, newest first: a transcript
/// directly in a project's folder (a subagent's is a folder down), never
/// one that ended in someone else's PR's worktree.
fn session_rows() -> Vec<SessionRow> {
    let (transcripts, repos) = read_cache();
    let mut sessions = recent_sessions(&transcripts);
    let projects = paths::claude_dir().join("projects");
    let mut rows = Vec::new();
    // One row a conversation: the newest transcript of an id kept in two
    // projects' folders that can be resumed, the sessions coming newest
    // first.
    let mut listed = HashSet::new();
    for s in &mut sessions {
        let path = PathBuf::from(&s.file);
        let top = path
            .parent()
            .and_then(Path::parent)
            .is_some_and(|p| p == projects);
        // Used within two weeks by its own last timestamp, not only by
        // the file's modified time, which an entry with none moves.
        let recent = s.active_ms() > (util::now_ms() - RECENT_MS) as f64;
        if !top || !recent || crate::trust::is_untrusted(Path::new(s.cwd())) {
            continue;
        }
        let Some(id) = path.file_stem().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        if listed.contains(&id) {
            continue;
        }
        if !s.entry.contains_key("prompt") {
            let p = prompt_in(&path, s.size());
            s.entry.insert("prompt".into(), json!(p));
        }
        if !s.entry.contains_key("turns") {
            s.entry.insert("turns".into(), json!(turns_in(&path)));
        }
        // Where it started is the transcript's own, kept; where it resumes
        // from is worked out on every look, since its folder can go and
        // come back.
        if !s.entry.contains_key("firstCwd") {
            let first = crate::cmd::claude_here::read_first_cwd(&path);
            s.entry
                .insert("firstCwd".into(), json!(first.unwrap_or_default()));
        }
        let first = s
            .entry
            .get("firstCwd")
            .and_then(Value::as_str)
            .filter(|f| !f.is_empty());
        let resume = path
            .parent()
            .and_then(|project| crate::cmd::claude_here::resume_dir(first, s.cwd(), project))
            .unwrap_or_default();
        // Nor one `claude --resume` can't find, no folder its project is
        // named from being there any more, as claude-here leaves it out,
        // nor one whose resuming would start Claude in an untrusted folder.
        if resume.is_empty() || crate::trust::is_untrusted(Path::new(&resume)) {
            continue;
        }
        // Its newest copy Claude can resume it from, an older one in a
        // folder still there standing in for one whose folder is gone.
        listed.insert(id.clone());
        rows.push(SessionRow {
            id,
            dir: s.cwd().to_string(),
            when: (s.active_ms() / 1000.0).floor() as i64,
            prompt: s
                .entry
                .get("prompt")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            turns: s.entry.get("turns").and_then(Value::as_u64).unwrap_or(0) as usize,
            resume,
        });
    }
    let kept: Map<String, Value> = sessions
        .into_iter()
        .map(|s| (s.file, Value::Object(s.entry)))
        .collect();
    write_cache(kept, repos);
    rows
}

/// How long ago, as the rows say it.
fn age(when: i64, now_secs: i64) -> String {
    let s = now_secs - when;
    if s < 3600 {
        format!("{}m ago", s.div_euclid(60))
    } else if s < 172_800 {
        format!("{}h ago", s.div_euclid(3600))
    } else {
        format!("{}d ago", s.div_euclid(86_400))
    }
}

fn inside(dir: &str, tree: &str) -> bool {
    dir == tree || dir.starts_with(&format!("{tree}/"))
}

/// The rows, and whether any repository was found to look in.
fn survey(dir: &Path, pane_dirs: &[PathBuf]) -> (Vec<Row>, bool) {
    let (transcripts, mut repos) = read_cache();
    let mut sessions = recent_sessions(&transcripts);
    let panes: Vec<String> = pane_dirs
        .iter()
        .map(|d| d.to_string_lossy().into_owned())
        .collect();

    let mut seen = HashSet::new();
    let dirs: Vec<String> = std::iter::once(dir.to_string_lossy().into_owned())
        .chain(panes.iter().cloned())
        .chain(sessions.iter().map(|s| s.cwd().to_string()))
        .filter(|d| seen.insert(d.clone()))
        .collect();
    let mut seen_repo = HashSet::new();
    let found: Vec<String> = dirs
        .iter()
        .filter(|d| Path::new(d).exists())
        .filter_map(|d| repo_of(d, &mut repos))
        .filter(|r| seen_repo.insert(r.clone()))
        .collect();

    let now_secs = util::now_secs();
    let mut rows = Vec::new();
    for repo in &found {
        let name = Path::new(repo)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        for (tree, branch) in worktrees_of(repo) {
            // A worktree is open when some pane is in it or below it.
            if panes.iter().any(|p| inside(p, &tree)) {
                continue;
            }
            // The newest session in this worktree, or in a folder below it.
            let session = sessions.iter_mut().find(|s| inside(s.cwd(), &tree));
            let (when, prompt) = match session {
                Some(s) => {
                    if !s.entry.contains_key("prompt") {
                        let p = prompt_in(Path::new(&s.file), s.size());
                        s.entry.insert("prompt".into(), json!(p));
                    }
                    let prompt = s
                        .entry
                        .get("prompt")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    ((s.active_ms() / 1000.0).floor() as i64, prompt)
                }
                None => {
                    let mtime = fs::metadata(&tree).map(|m| mtime_ms(&m)).unwrap_or(0.0);
                    (
                        (mtime / 1000.0).floor() as i64,
                        "(no recent Claude session)".to_string(),
                    )
                }
            };
            rows.push(Row {
                when,
                label: format!("{name} {branch}"),
                age: age(when, now_secs),
                tree,
                repo: repo.clone(),
                branch,
                prompt,
            });
        }
    }
    rows.sort_by_key(|r| std::cmp::Reverse(r.when));

    // Only what this look saw is kept, so files that age out or are deleted
    // leave the cache with them.
    let kept_transcripts: Map<String, Value> = sessions
        .into_iter()
        .map(|s| (s.file, Value::Object(s.entry)))
        .collect();
    let kept_repos: Map<String, Value> = dirs
        .iter()
        .filter_map(|d| repos.get(d).map(|v| (d.clone(), v.clone())))
        .collect();
    write_cache(kept_transcripts, kept_repos);
    (rows, !found.is_empty())
}

/// Every pane's directory, from the running server; none outside one.
fn pane_dirs() -> Vec<PathBuf> {
    let Ok(text) = crate::mux::socket::ask_ok(&["list"]) else {
        return Vec::new();
    };
    let Ok(list) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    let mut dirs = Vec::new();
    for tab in list
        .get("sessions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|s| {
            s.get("tabs")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
    {
        for pane in tab
            .get("panes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let sidebar = pane
                .pointer("/options/@sidebar")
                .and_then(Value::as_str)
                .is_some_and(|v| !v.is_empty() && v != "0");
            if let (false, Some(cwd)) = (sidebar, pane.get("cwd").and_then(Value::as_str)) {
                dirs.push(PathBuf::from(cwd));
            }
        }
    }
    dirs
}

pub fn main(args: Vec<String>) -> ExitCode {
    let usage = || {
        eprintln!("Usage: wt-revive [--json] [<dir>] | wt-revive --sessions [--json]");
        ExitCode::from(1)
    };
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return usage();
    }
    let json = args.iter().any(|a| a == "--json");
    if args.iter().any(|a| a == "--sessions") {
        for row in session_rows() {
            if json {
                println!("{}", util::json_compact(&row));
            } else {
                println!(
                    "{}\t{}\t{}",
                    row.id,
                    paths::tilde(Path::new(&row.dir)),
                    row.prompt
                );
            }
        }
        return ExitCode::SUCCESS;
    }
    // --list, from before the rows were all it printed, is still taken.
    let positional: Vec<&String> = args
        .iter()
        .filter(|a| *a != "--list" && *a != "--json")
        .collect();
    if positional.len() > 1 {
        return usage();
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let dir = match positional.first() {
        Some(d) => crate::worktree::normalize(&cwd.join(d)),
        None => cwd,
    };
    let (rows, any_repo) = survey(&dir, &pane_dirs());
    if !any_repo {
        if json {
            return ExitCode::SUCCESS;
        }
        eprintln!("No git repositories found in your tabs or recent Claude sessions.");
        return ExitCode::from(1);
    }
    if json {
        for row in &rows {
            println!("{}", util::json_compact(row));
        }
        return ExitCode::SUCCESS;
    }
    if rows.is_empty() {
        eprintln!("Every worktree already has a tab open.");
        return ExitCode::from(1);
    }
    for row in &rows {
        println!("{}\t{}\t{}", row.label, row.age, row.prompt);
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages_as_the_rows_say_them() {
        assert_eq!(age(1000 - 300, 1000), "5m ago");
        assert_eq!(age(0, 7200), "2h ago");
        assert_eq!(age(0, 3 * 86_400), "3d ago");
    }

    #[test]
    fn prompts_are_what_you_typed() {
        let text = [
            r#"{"type":"user","message":{"content":"a long enough prompt\nhere"}}"#,
            r#"{"type":"user","isMeta":true,"message":{"content":"meta, not yours at all"}}"#,
            r#"{"type":"user","message":{"content":"<command-name>/clear</command-name>"}}"#,
            r#"{"type":"user","message":{"content":"<command-message>shape</command-message>\n<command-name>/shape</command-name>"}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result"}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"text","text":"look at this screenshot"},{"type":"image"}]}}"#,
        ]
        .join("\n");
        assert_eq!(
            prompts_in(&text),
            vec!["a long enough prompt here", "look at this screenshot"]
        );
        let lines: Vec<&str> = text.split('\n').collect();
        assert_eq!(
            lines.iter().filter(|l| typed_command(l)).count(),
            2,
            "the slash commands you typed count as turns"
        );
    }
}
