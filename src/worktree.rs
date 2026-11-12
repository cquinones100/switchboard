//! What the commands that open a worktree for a piece of work share
//! (`worktree-from-ticket`, `worktree-from-note`, `worktree-from-branch`,
//! `worktree-from-pr`, `worktree-resume`, and those that find one again): the
//! branch's slug, the main clone, the local settings linked in, the worktree
//! made or reused, refused while Claude already works in it, git's list of
//! worktrees, fetching a branch, the arguments every one of them takes
//! (`--cd-file`, `--no-claude`), the shell function each prints for `init`,
//! starting Claude in the worktree, and a pull request's details from GitHub.
//! Ported from lib/worktree.js, and from the helpers each bin/worktree-*
//! script kept a copy of. Errors are returned, for each command's own `fail`
//! to print.

use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::LazyLock;

use anyhow::{Result, anyhow, bail};
use regex::Regex;
use serde_json::{Value, json};

use crate::integrations::github::{self, Repo};
use crate::{paths, requests, util};

/// Where a new worktree goes, under the main clone, unless a path is given.
pub const DEFAULT_WORKTREE_DIR: &str = ".claude/worktrees";

/// A branch-name slug: lowercase letters, digits and single hyphens.
pub fn slugify(text: &str) -> String {
    let lower = text.trim().to_lowercase();
    let kept: String = lower
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c.is_whitespace() || *c == '-')
        .collect();
    let mut out = String::new();
    let mut last_hyphen = false;
    for c in kept.chars() {
        let c = if c.is_whitespace() { '-' } else { c };
        if c == '-' {
            if !last_hyphen {
                out.push('-');
            }
            last_hyphen = true;
        } else {
            out.push(c);
            last_hyphen = false;
        }
    }
    out.trim_matches('-').to_string()
}

/// git's output, trimmed; an error when git fails.
pub fn git_output(args: &[&str]) -> Result<String> {
    Ok(util::run("git", args)?.trim().to_string())
}

/// Runs git with its output thrown away; an error when it fails.
pub fn git_run(args: &[&str]) -> Result<()> {
    let status = Command::new("git")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("git {} failed", args.first().unwrap_or(&"")))
    }
}

/// The main clone: `--git-common-dir` rather than `--show-toplevel`, since
/// inside an existing worktree the latter is that worktree, and new worktrees
/// belong beside the main repository.
pub fn repo_root() -> Result<PathBuf> {
    let common = git_output(&["rev-parse", "--git-common-dir"])
        .map_err(|_| anyhow!("not inside a git repository"))?;
    let common = std::env::current_dir()?.join(common);
    let resolved = common.parent().map(Path::to_path_buf).unwrap_or(common);
    Ok(normalize(&resolved))
}

/// A path with `.` and `..` resolved without touching the disk, as Node's
/// `path.resolve` does.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Claude Code reads `.claude/settings.local.json` from the worktree root,
/// not the main clone, and git never copies it because it is gitignored.
/// Symlinking the main clone's file keeps one source of truth for personal
/// permissions and the auto mode environment profile. Skipped when the
/// worktree already has its own.
pub fn link_local_settings(root: &Path, worktree: &Path) -> Result<()> {
    let source = root.join(".claude/settings.local.json");
    let target = worktree.join(".claude/settings.local.json");
    if !source.exists() || target.exists() {
        return Ok(());
    }
    if let Some(dir) = target.parent() {
        fs::create_dir_all(dir)?;
    }
    std::os::unix::fs::symlink(&source, &target)?;
    Ok(())
}

/// The title the session picker shows: a name given with /rename, else the
/// title Claude Code generated from the first prompt. Both live in files
/// Claude Code does not document, so anything unreadable leaves it blank.
pub fn session_title(session: &Value) -> String {
    let name = session.get("name").and_then(Value::as_str);
    let source = session.get("nameSource").and_then(Value::as_str);
    if let (Some(name), Some(source)) = (name, source)
        && !name.is_empty()
        && !source.is_empty()
        && source != "derived"
    {
        return name.to_string();
    }
    let id = session
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or("");
    let projects = paths::claude_dir().join("projects");
    let transcript = fs::read_dir(&projects).ok().and_then(|dirs| {
        dirs.flatten()
            .map(|d| d.path().join(format!("{id}.jsonl")))
            .find(|f| f.exists())
    });
    let Some(transcript) = transcript else {
        return name.unwrap_or("").to_string();
    };
    let text = fs::read_to_string(transcript).unwrap_or_default();
    text.split('\n')
        .filter(|line| line.contains(r#""type":"ai-title""#))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|v| {
            v.get("aiTitle")
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
                .map(str::to_string)
        })
        .next_back()
        .unwrap_or_else(|| name.unwrap_or("").to_string())
}

/// The running Claude sessions working in a directory or below it. Claude
/// Code writes `~/.claude/sessions/<pid>.json` for every running session,
/// naming its working directory; a file whose process is gone is left over
/// from a session that ended without cleaning up, and is ignored.
pub fn live_sessions_in(worktree: &Path) -> Vec<Value> {
    let root = worktree.to_string_lossy().into_owned();
    live_sessions()
        .filter(|s| {
            let cwd = s.get("cwd").and_then(Value::as_str).unwrap_or("");
            !cwd.is_empty() && (cwd == root || cwd.starts_with(&format!("{root}/")))
        })
        .collect()
}

/// Whether the Claude session with this id is running, wherever it works.
pub fn session_runs(session_id: &str) -> bool {
    live_sessions().any(|s| s.get("sessionId").and_then(Value::as_str) == Some(session_id))
}

// The session files in `~/.claude/sessions` that name a session and whose
// process is alive.
fn live_sessions() -> impl Iterator<Item = Value> {
    let dir = paths::claude_dir().join("sessions");
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
        .filter_map(|e| util::read_json(e.path()))
        .filter(|s| {
            s.get("sessionId").is_some_and(util::truthy)
                && s.get("pid")
                    .and_then(Value::as_i64)
                    .is_some_and(util::is_alive)
        })
}

/// The tab a process runs in, as `<session>:<number> <name>`, from the
/// running server's panes and their process trees, for the refusal to say
/// where the session already working is, as `ensureWorktree` named its tmux
/// window. `None` with no server, or when no pane holds it.
fn tab_holding(pid: u32) -> Option<String> {
    let list: Value = serde_json::from_str(&crate::mux::socket::ask_ok(&["list"]).ok()?).ok()?;
    let under = |root: u32| {
        let mut stack = vec![root];
        let mut seen = 0;
        while let Some(p) = stack.pop() {
            if p == pid {
                return true;
            }
            seen += 1;
            if seen > 4096 {
                break;
            }
            stack.extend(crate::mux::procinfo::children_of(p));
        }
        false
    };
    for session in list["sessions"].as_array()? {
        for tab in session["tabs"].as_array().into_iter().flatten() {
            let holds = tab["panes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| p["pid"].as_u64())
                .any(|root| under(root as u32));
            if holds {
                return Some(format!(
                    "{}:{} {}",
                    session["name"].as_str().unwrap_or(""),
                    tab["index"].as_i64().unwrap_or(0),
                    tab["label"].as_str().unwrap_or("")
                ));
            }
        }
    }
    None
}

/// What a worktree is for, in the refusal's words:`work` ("ticket", "item")
/// and `command` ("wt", "wtn"); `start` is the commit a new branch is made
/// from, with no upstream, else the current HEAD.
pub struct Purpose<'a> {
    pub work: &'a str,
    pub command: &'a str,
    pub start: Option<&'a str>,
}

/// Makes the worktree, or reuses it. Idempotent so rerunning on the same work
/// reopens the worktree instead of erroring: the directory is reused if it is
/// there, and the branch if it is there.
pub fn ensure_worktree(worktree: &Path, branch: &str, purpose: &Purpose) -> Result<()> {
    if worktree.exists() {
        // Two sessions on one piece of work edit the same files and push the
        // same branch, so a worktree that already has Claude running in it is
        // left to that one.
        let sessions = live_sessions_in(worktree);
        if !sessions.is_empty() {
            let lines: Vec<String> = sessions
                .iter()
                .map(|s| {
                    let title = session_title(s);
                    let id = s.get("sessionId").and_then(Value::as_str).unwrap_or("");
                    let tab = s
                        .get("pid")
                        .and_then(Value::as_u64)
                        .and_then(|pid| tab_holding(pid as u32))
                        .map(|tab| format!("\n  Tab:     {tab}"))
                        .unwrap_or_default();
                    format!(
                        "  Session: {id}\n  Title:   {}{tab}",
                        if title.is_empty() {
                            "(none yet)"
                        } else {
                            &title
                        }
                    )
                })
                .collect();
            bail!(
                "Claude is already working on this {}, so {} will not start another session.\n{}\nSwitch to that session, or end it and run {} again.",
                purpose.work,
                purpose.command,
                lines.join("\n\n"),
                purpose.command
            );
        }
        // A directory of the same name on another branch is another piece of
        // work, never taken for this one.
        let path = worktree.to_string_lossy();
        let checked_out =
            git_output(&["-C", &path, "branch", "--show-current"]).unwrap_or_default();
        if checked_out != branch {
            let on = if checked_out.is_empty() {
                "no branch of this repository".to_string()
            } else {
                format!("branch {checked_out}")
            };
            bail!(
                "{} is there already, on {on}, not {branch}.",
                worktree.display()
            );
        }
        eprintln!("Worktree already exists, reusing it.");
        return Ok(());
    }

    let branch_exists = git_run(&[
        "show-ref",
        "--verify",
        "--quiet",
        &format!("refs/heads/{branch}"),
    ])
    .is_ok();
    let path = worktree.to_string_lossy().into_owned();
    let args: Vec<&str> = if branch_exists {
        vec!["worktree", "add", &path, branch]
    } else if let Some(start) = purpose.start {
        vec!["worktree", "add", "--no-track", "-b", branch, &path, start]
    } else {
        vec!["worktree", "add", "-b", branch, &path]
    };
    // git's stdout to stderr, so it can't pollute anything a caller parses.
    let status = Command::new("git")
        .args(&args)
        .stdin(Stdio::inherit())
        .stdout(std::io::stderr())
        .stderr(Stdio::inherit())
        .status();
    if !status.is_ok_and(|s| s.success()) {
        bail!("git worktree creation failed (see output above)");
    }
    Ok(())
}

// ── Running in a repository ─────────────────────────────────────────────────

/// git's output in `dir`, trimmed; an error when git fails. Its stderr is
/// thrown away.
pub fn git_at(dir: &Path, args: &[&str]) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).args(args);
    Ok(util::run_command(&mut cmd)?.trim().to_string())
}

/// Runs git in `dir`, its output thrown away; whether it succeeded.
pub fn git_ok(dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Runs git in `dir` with its output on this program's stderr, so it can't
/// pollute anything a caller parses, and its own errors shown or not;
/// whether it succeeded.
pub fn git_loud(dir: &Path, args: &[&str], show_errors: bool) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .stdout(std::io::stderr())
        .stderr(if show_errors {
            Stdio::inherit()
        } else {
            Stdio::null()
        })
        .status()
        .is_ok_and(|s| s.success())
}

/// The main clone of the repository `dir` is in, from `--git-common-dir`,
/// or `None` outside one.
pub fn repo_root_of(dir: &Path) -> Option<PathBuf> {
    let common = git_at(dir, &["rev-parse", "--git-common-dir"]).ok()?;
    let common = dir.join(common);
    Some(normalize(
        &common.parent().map(Path::to_path_buf).unwrap_or(common),
    ))
}

/// One of git's worktrees, from `git worktree list --porcelain`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    /// The branch checked out, without `refs/heads/`; `None` when detached.
    pub branch: Option<String>,
    pub detached: bool,
    /// Its directory is gone, and git would prune it.
    pub prunable: bool,
    pub bare: bool,
}

/// Every worktree of the repository at `root`, the main clone first.
pub fn worktrees(root: &Path) -> Result<Vec<Worktree>> {
    let porcelain = git_at(root, &["worktree", "list", "--porcelain"])?;
    Ok(parse_worktrees(&porcelain))
}

/// `git worktree list --porcelain`, read: records are blank-line separated,
/// `worktree <path>` opens each one, and `branch refs/heads/<name>` appears
/// in it only when a branch is checked out.
pub fn parse_worktrees(porcelain: &str) -> Vec<Worktree> {
    porcelain
        .split("\n\n")
        .filter_map(|record| {
            let lines: Vec<&str> = record.lines().collect();
            let path = lines.iter().find_map(|l| l.strip_prefix("worktree "))?;
            let word = |w: &str| {
                lines
                    .iter()
                    .any(|l| *l == w || l.starts_with(&format!("{w} ")))
            };
            Some(Worktree {
                path: PathBuf::from(path),
                branch: lines
                    .iter()
                    .find_map(|l| l.strip_prefix("branch refs/heads/"))
                    .map(str::to_string),
                detached: word("detached"),
                prunable: word("prunable"),
                bare: word("bare"),
            })
        })
        .collect()
}

/// The worktree a branch is checked out in, if any.
pub fn holding_branch(root: &Path, branch: &str) -> Option<PathBuf> {
    worktrees(root)
        .ok()?
        .into_iter()
        .find(|w| w.branch.as_deref() == Some(branch))
        .map(|w| w.path)
}

/// The repository's remotes, in git's order.
pub fn remotes(root: &Path) -> Vec<String> {
    git_at(root, &["remote"])
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// `origin`, else the first remote; `None` for a repository with none.
pub fn default_remote(root: &Path) -> Option<String> {
    let all = remotes(root);
    if all.iter().any(|r| r == "origin") {
        return Some("origin".into());
    }
    all.into_iter().next()
}

/// Whether a local branch of that name exists.
pub fn branch_exists(root: &Path, branch: &str) -> bool {
    git_ok(
        root,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
}

/// The host a remote's URL reaches, for the request audit: a URL with a
/// scheme (not `file://`) or git's scp form (`host:path`, no `/` before the
/// colon); `None` for a path on this computer.
pub fn remote_host(url: &str) -> Option<String> {
    static SCHEME: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^[A-Za-z][A-Za-z0-9+.-]*://").unwrap());
    static SCP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[^/]+:").unwrap());
    if url.starts_with("file:") {
        return None;
    }
    let rest = if SCHEME.is_match(url) {
        url.split_once("://").map(|(_, r)| r)?
    } else if SCP.is_match(url) {
        url
    } else {
        return None;
    };
    let rest = match rest.find(['@', '/']) {
        Some(at) if rest.as_bytes()[at] == b'@' => &rest[at + 1..],
        _ => rest,
    };
    let host = rest.split(['/', ':']).next().unwrap_or("");
    (!host.is_empty()).then(|| host.to_string())
}

/// Counts a fetch from a remote in the request audit, under its host
/// (GitHub for github.com), and not at all for a remote on this computer.
pub fn count_fetch(root: &Path, remote: &str, source: &str) {
    let url = git_at(root, &["remote", "get-url", remote]).unwrap_or_default();
    if let Some(host) = remote_host(&url) {
        let service = if host == "github.com" || host == "gist.github.com" {
            "GitHub".to_string()
        } else {
            host
        };
        requests::count(&service, "git", source);
    }
}

/// Fetches one branch from a remote with an explicit refspec, so the
/// remote-tracking ref is always written whatever the remote's configured
/// refspec is. Whether the remote has the branch.
pub fn fetch_branch(root: &Path, remote: &str, branch: &str, source: &str) -> bool {
    count_fetch(root, remote, source);
    git_loud(
        root,
        &[
            "fetch",
            remote,
            &format!("+refs/heads/{branch}:refs/remotes/{remote}/{branch}"),
        ],
        false,
    )
}

/// The commit a new branch starts from: origin's default branch (from
/// `refs/remotes/origin/HEAD`, else main or master) as it is now, fetched
/// first, as `refs/remotes/origin/<name>` (in full, so no local ref named
/// `origin/<name>` stands in for it); the main clone's HEAD only with no origin. A
/// fetch that fails is an error rather than the clone's HEAD, whose unpushed
/// commits would otherwise land with the new branch's work.
pub fn default_start(root: &Path, source: &str) -> Result<String, String> {
    if !remotes(root).iter().any(|r| r == "origin") {
        return git_at(root, &["rev-parse", "HEAD"]).map_err(|e| e.to_string());
    }
    let fetched = |base: &str| fetch_branch(root, "origin", base, source);
    let named = git_at(
        root,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .ok()
    .map(|n| n.strip_prefix("origin/").unwrap_or(&n).to_string());
    let base = match &named {
        Some(name) => fetched(name).then(|| name.clone()),
        None => ["main", "master"]
            .into_iter()
            .find(|b| fetched(b))
            .map(str::to_string),
    };
    match base {
        Some(base) => Ok(format!("refs/remotes/origin/{base}")),
        None => Err(format!(
            "couldn't fetch origin's {}, which the new branch starts from",
            named.as_deref().unwrap_or("main or master")
        )),
    }
}

/// The upstream written by hand rather than with `worktree add --track`:
/// `--track` insists the start point be covered by the remote's fetch
/// refspec, which a single-branch or partial clone's is not.
pub fn set_upstream(root: &Path, branch: &str, remote: &str, remote_branch: &str) {
    git_ok(
        root,
        &["config", &format!("branch.{branch}.remote"), remote],
    );
    git_ok(
        root,
        &[
            "config",
            &format!("branch.{branch}.merge"),
            &format!("refs/heads/{remote_branch}"),
        ],
    );
}

// ── What is asked for ───────────────────────────────────────────────────────

static TICKET_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z][A-Za-z0-9]*-\d+$").unwrap());
static ISSUE_URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/issue/([A-Za-z][A-Za-z0-9]*-\d+)").unwrap());
static PR_URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"github\.com/([^/]+)/([^/]+)/pull/(\d+)").unwrap());
static PR_NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^#?(\d+)$").unwrap());

/// Whether the text is a Linear ticket: its identifier or an issue URL.
pub fn looks_like_ticket(input: &str) -> bool {
    ISSUE_URL.is_match(input) || TICKET_ID.is_match(input)
}

/// The ticket's identifier as written (`abc-123` stays so), from an
/// identifier or an issue URL.
pub fn ticket_from(input: &str) -> Option<String> {
    if let Some(m) = ISSUE_URL.captures(input) {
        return Some(m[1].to_string());
    }
    TICKET_ID.is_match(input).then(|| input.to_string())
}

/// Whether the text is a pull request: its number (`123`, `#123`) or URL.
pub fn looks_like_pr(input: &str) -> bool {
    PR_URL.is_match(input) || PR_NUMBER.is_match(input)
}

/// A pull request's number, and the `owner/name` a URL names.
pub fn pr_from(input: &str) -> Option<(u64, Option<String>)> {
    if let Some(m) = PR_URL.captures(input) {
        return Some((m[3].parse().ok()?, Some(format!("{}/{}", &m[1], &m[2]))));
    }
    let m = PR_NUMBER.captures(input)?;
    Some((m[1].parse().ok()?, None))
}

/// Whether `text` holds `token` bounded on both sides, so ABC-12 doesn't
/// match abc-123: the identifier is a token inside a longer name, not the
/// whole of it. Both lowercased, as the names `wt` and `wtpr` make are.
pub fn mentions(text: &str, token: &str) -> bool {
    let text = text.to_lowercase();
    let token = token.to_lowercase();
    if token.is_empty() {
        return false;
    }
    let bounded =
        |c: Option<char>| c.is_none_or(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit()));
    text.match_indices(&token).any(|(at, _)| {
        bounded(text[..at].chars().next_back()) && bounded(text[at + token.len()..].chars().next())
    })
}

/// What a worktree command was given besides its own flags: `--cd-file
/// <path>` (where the shell function reads the worktree back), `--no-claude`
/// (the function starts Claude itself, after its cd), each value flag's
/// values in order, and the rest.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Given {
    pub cd_file: Option<String>,
    pub no_claude: bool,
    pub values: Vec<(String, String)>,
    pub positional: Vec<String>,
    pub help: bool,
}

impl Given {
    /// Reads the arguments. Each value flag in `value_flags` and its value
    /// come out first, wherever they are, so text that reads as a flag
    /// (`--prompt --help`) is only ever its value; then `--cd-file` and its
    /// path, `--no-claude` and `-h`/`--help`. The error names a flag given
    /// no value.
    pub fn parse(args: &[String], value_flags: &[&str]) -> Result<Given, String> {
        let mut given = Given::default();
        let mut rest = Vec::new();
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            if value_flags.contains(&a.as_str()) {
                let Some(v) = args.get(i + 1) else {
                    return Err(a.clone());
                };
                given.values.push((a.clone(), v.clone()));
                i += 2;
                continue;
            }
            rest.push(a.clone());
            i += 1;
        }
        given.help = rest.iter().any(|a| a == "--help" || a == "-h");
        let mut j = 0;
        while j < rest.len() {
            match rest[j].as_str() {
                "--cd-file" => {
                    let Some(path) = rest.get(j + 1) else {
                        return Err("--cd-file".into());
                    };
                    given.cd_file = Some(path.clone());
                    j += 2;
                    continue;
                }
                "--no-claude" => given.no_claude = true,
                other => given.positional.push(other.to_string()),
            }
            j += 1;
        }
        Ok(given)
    }

    /// Every value given for a flag, in order.
    pub fn all(&self, flag: &str) -> Vec<&str> {
        self.values
            .iter()
            .filter(|(f, _)| f == flag)
            .map(|(_, v)| v.as_str())
            .collect()
    }

    /// The last value given for a flag.
    pub fn last(&self, flag: &str) -> Option<&str> {
        self.all(flag).last().copied()
    }
}

/// Text made one line, its runs of whitespace one space, as a prompt the
/// shell function reads back as a line of the cd-file.
pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A word quoted for a POSIX shell.
pub fn shell_quote(word: &str) -> String {
    if !word.is_empty()
        && word
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./=:@%+,".contains(&b))
    {
        return word.to_string();
    }
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// This program as a shell runs it, `'<switchboard>' <name>`: the binary's
/// own path, so the shell function works whether or not `<name>` is linked
/// onto the PATH.
pub fn self_command(name: &str) -> String {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "switchboard".into());
    format!("{} {name}", shell_quote(&exe))
}

/// For `<command> init <zsh|bash>`: `None` when the arguments aren't an
/// `init`, else whether they name a shell it writes for.
pub fn init_shell(args: &[String]) -> Option<bool> {
    if args.first().map(String::as_str) != Some("init") {
        return None;
    }
    Some(matches!(
        args.get(1).map(String::as_str),
        Some("zsh" | "bash")
    ))
}

/// The shell a tab that starts work runs: yours (`$SHELL`) when it is zsh
/// or bash, which the functions `init` writes are for; else zsh or bash,
/// whichever is there.
pub fn work_shell() -> String {
    let yours = std::env::var("SHELL").unwrap_or_default();
    let name = Path::new(&yours)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if matches!(name.as_str(), "zsh" | "bash") {
        return yours;
    }
    ["zsh", "bash"]
        .iter()
        .find_map(|s| util::which(s))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "bash".into())
}

/// A new tab's command that runs `script` with `args` (its `"$@"`) in an
/// interactive shell, which reads your rc for the PATH Claude is on, and
/// stays a shell when it ends, in whatever directory it left the shell in:
/// a worktree's, after `wt`. The arguments are words of their own, never
/// read as shell, so a ticket or an idea can hold anything. tmux's version
/// typed `wt <ticket>` into the new tab's shell, which raced the shell's
/// start and lost its Enter when the prompt wasn't ready (shape-batch's
/// comment); a command has nothing to race.
pub fn tab_command(script: &str, name: &str, args: &[String]) -> Vec<String> {
    let shell = work_shell();
    let line = format!("{script}; exec {} -i", shell_quote(&shell));
    let mut command = vec![shell, "-i".into(), "-c".into(), line, name.to_string()];
    command.extend(args.iter().cloned());
    command
}

/// A new tab's command that defines a worktree command's shell function
/// from its `init` (`wt` from `worktree-from-ticket`), so it is there
/// whether or not your rc defines it, and runs it with `args`.
pub fn tab_running(program: &str, function: &str, args: &[String]) -> Vec<String> {
    let shell = work_shell();
    let kind = if shell.ends_with("bash") {
        "bash"
    } else {
        "zsh"
    };
    let script = format!(
        "eval \"$({} init {kind})\"; {function} \"$@\"",
        self_command(program)
    );
    tab_command(&script, function, args)
}

/// Says what went wrong as every worktree command does, `Error: ...`, and
/// gives the exit code 1.
pub fn fail(message: &str) -> ExitCode {
    eprintln!("Error: {message}");
    ExitCode::from(1)
}

/// The tab this runs in takes up a worktree's work, so a landing left there
/// before (`@tab_landed`) no longer files it under Merged. Outside a
/// Switchboard pane it does nothing.
fn take_up_work() {
    if crate::mux::socket::in_pane() {
        let _ = crate::mux::socket::ask(&["set", "-u", crate::sidebar::model::LANDED]);
    }
}

/// Writes the cd-file the shell function reads back, which then starts
/// Claude in the worktree itself, so the tab takes up its work here.
pub fn write_cd_file(path: &str, contents: &str) -> Result<()> {
    take_up_work();
    fs::write(path, contents).map_err(|e| anyhow!("couldn't write {path}: {e}"))
}

/// Starts `program` (Claude, or `claude-here`) in the worktree with these
/// arguments, the terminal its own, and gives its exit code; one that isn't
/// on the PATH is said, with where the worktree is, and is no failure. The
/// tab it runs in takes up this work (`take_up_work`).
pub fn run_in(program: &str, args: &[String], dir: &Path) -> ExitCode {
    take_up_work();
    match Command::new(program).args(args).current_dir(dir).status() {
        Ok(status) => match status.code() {
            Some(0) | None => ExitCode::SUCCESS,
            Some(code) => ExitCode::from(code.clamp(1, 255) as u8),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!(
                "\n{program} not found on PATH. The worktree is ready at:\n  {}",
                dir.display()
            );
            ExitCode::SUCCESS
        }
        Err(e) => fail(&format!("couldn't start {program}: {e}")),
    }
}

/// Asks for one of `count` numbered choices on a line of stdin, the prompt
/// on stderr: Enter alone, or stdin closed, is the first; anything else is
/// asked again. Gives its index. Numbered rather than a raw-mode picker,
/// since the cases it is for (two worktrees or tabs matching) are rare.
pub fn choose_numbered(prompt: &str, count: usize) -> usize {
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    loop {
        eprint!("{prompt}");
        let _ = std::io::stderr().flush();
        let Some(Ok(answer)) = lines.next() else {
            return 0;
        };
        let answer = answer.trim();
        if answer.is_empty() {
            return 0;
        }
        match answer.parse::<usize>() {
            Ok(n) if (1..=count).contains(&n) => return n - 1,
            _ => eprintln!("enter a number between 1 and {count}"),
        }
    }
}

// ── Pull requests ───────────────────────────────────────────────────────────

/// A pull request as `gh pr view --json` gave the worktree commands it:
/// enough to check out its branch (a fork's included), to say whose it is
/// and how big, and, for codex-review, its description and base.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrDetails {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub url: String,
    /// "OPEN", "MERGED" or "CLOSED".
    pub state: String,
    pub author: Option<String>,
    pub base_ref_name: String,
    pub head_ref_name: String,
    /// The fork's (or this repository's) name, `None` when the fork is gone.
    pub head_repository: Option<String>,
    pub head_owner: Option<String>,
    pub is_cross_repository: bool,
    pub maintainer_can_modify: bool,
    /// When the author's GitHub account was made (RFC 3339), when it is a
    /// person's: how long they have been on GitHub, for someone else's PR.
    pub author_since: Option<String>,
    /// Its size: the files it changes, and the lines it adds and takes out.
    pub changed_files: u64,
    pub additions: u64,
    pub deletions: u64,
}

const PR_DETAILS: &str = "query($owner: String!, $name: String!, $number: Int!) { \
     repository(owner: $owner, name: $name) { pullRequest(number: $number) { \
     number title body url state author { login ... on User { createdAt } } \
     baseRefName headRefName headRepository { name } headRepositoryOwner { login } \
     isCrossRepository maintainerCanModify changedFiles additions deletions } } }";

/// A pull request's details, or `None` when the repository has no such PR.
pub async fn pr_details(repo: &Repo, number: u64, source: &str) -> Result<Option<PrDetails>> {
    let result = github::graphql(
        PR_DETAILS,
        json!({ "owner": repo.owner, "name": repo.name, "number": number }),
        source,
    )
    .await;
    let data = match result {
        Ok(data) => data,
        // A number with no PR is GitHub's NOT_FOUND on the pullRequest
        // field: an answer, not a failure (github::pr_by_number).
        Err(e)
            if crate::net::kind_of(&e) == Some(&crate::net::ErrorKind::NotFound)
                && e.to_string().contains("PullRequest") =>
        {
            return Ok(None);
        }
        Err(e) => return Err(e),
    };
    Ok(data
        .pointer("/repository/pullRequest")
        .filter(|pr| pr.is_object())
        .map(pr_details_from))
}

/// The details out of GitHub's `pullRequest`.
pub fn pr_details_from(pr: &Value) -> PrDetails {
    let text = |path: &str| {
        pr.pointer(path)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let some = |path: &str| {
        pr.pointer(path)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    PrDetails {
        number: pr.get("number").and_then(Value::as_u64).unwrap_or(0),
        title: text("/title"),
        body: text("/body"),
        url: text("/url"),
        state: text("/state"),
        author: some("/author/login"),
        base_ref_name: text("/baseRefName"),
        head_ref_name: text("/headRefName"),
        head_repository: some("/headRepository/name"),
        head_owner: some("/headRepositoryOwner/login"),
        is_cross_repository: pr
            .get("isCrossRepository")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        maintainer_can_modify: pr
            .get("maintainerCanModify")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        author_since: some("/author/createdAt"),
        changed_files: pr.get("changedFiles").and_then(Value::as_u64).unwrap_or(0),
        additions: pr.get("additions").and_then(Value::as_u64).unwrap_or(0),
        deletions: pr.get("deletions").and_then(Value::as_u64).unwrap_or(0),
    }
}

/// Who the GitHub token belongs to, or `None` when it can't be told.
pub async fn viewer_login(source: &str) -> Option<String> {
    let data = github::graphql("query { viewer { login } }", json!({}), source)
        .await
        .ok()?;
    data.pointer("/viewer/login")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The `owner/name` a remote URL points at on GitHub, lowercased, or `None`.
pub fn github_slug(url: &str) -> Option<String> {
    github::parse_remote(url).map(|r| format!("{}/{}", r.owner, r.name).to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_worktree_list_is_read_record_by_record() {
        let porcelain = "worktree /r\nHEAD abc\nbranch refs/heads/main\n\nworktree /r/.claude/worktrees/x\nHEAD def\ndetached\n\nworktree /gone\nHEAD 123\nbranch refs/heads/old\nprunable gitdir file points to non-existent location\n";
        let all = parse_worktrees(porcelain);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].branch.as_deref(), Some("main"));
        assert!(all[1].detached && all[1].branch.is_none());
        assert!(all[2].prunable);
    }

    #[test]
    fn hosts_of_remotes_for_the_audit() {
        assert_eq!(
            remote_host("git@github.com:o/r.git").as_deref(),
            Some("github.com")
        );
        assert_eq!(
            remote_host("https://x:y@gitlab.com/o/r").as_deref(),
            Some("gitlab.com")
        );
        assert_eq!(
            remote_host("ssh://git@host.example:22/o/r").as_deref(),
            Some("host.example")
        );
        assert_eq!(remote_host("/tmp/origin.git"), None);
        assert_eq!(remote_host("file:///tmp/origin.git"), None);
    }

    #[test]
    fn tickets_and_prs_as_written() {
        assert_eq!(ticket_from("abc-12").as_deref(), Some("abc-12"));
        assert_eq!(
            ticket_from("https://linear.app/w/issue/ENG-5/title").as_deref(),
            Some("ENG-5")
        );
        assert_eq!(ticket_from("eng 5"), None);
        assert_eq!(pr_from("#12"), Some((12, None)));
        assert_eq!(
            pr_from("https://github.com/o/r/pull/7/files"),
            Some((7, Some("o/r".into())))
        );
        assert!(!looks_like_pr("12a"));
    }

    #[test]
    fn mentions_are_bounded_on_both_sides() {
        assert!(mentions("abc-12-fix", "abc-12"));
        assert!(mentions("pr-7-feat", "pr-7"));
        assert!(!mentions("abc-123-fix", "abc-12"));
        assert!(!mentions("xabc-12", "abc-12"));
        assert!(mentions("ABC-12", "abc-12"));
    }

    #[test]
    fn flags_with_values_come_out_first() {
        let args: Vec<String> = [
            "--prompt",
            "--help",
            "--cd-file",
            "/f",
            "ABC-1",
            "--no-claude",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let g = Given::parse(&args, &["--prompt"]).unwrap();
        assert_eq!(g.all("--prompt"), vec!["--help"]);
        assert!(!g.help);
        assert_eq!(g.cd_file.as_deref(), Some("/f"));
        assert!(g.no_claude);
        assert_eq!(g.positional, vec!["ABC-1".to_string()]);
        assert_eq!(
            Given::parse(&["--prompt".into()], &["--prompt"]),
            Err("--prompt".into())
        );
    }

    #[test]
    fn words_are_quoted_for_the_shell() {
        assert_eq!(shell_quote("/usr/bin/x"), "/usr/bin/x");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn slugs_are_lowercase_words_joined_by_hyphens() {
        assert_eq!(slugify("  Fix the Thing: now!  "), "fix-the-thing-now");
        assert_eq!(slugify("a -- b"), "a-b");
        assert_eq!(slugify("-Café-"), "caf");
    }
}
