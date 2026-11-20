//! Your files from a git repository: with Settings' `files_from_repo` on
//! and `~/.config/switchboard/files.json` naming one (`{"repo": "<url>",
//! "dir": "switchboard"}`), every file of your own is read from and written
//! to a clone of it, `~/.local/share/switchboard/files/<dir>`, which the
//! server pulls as it starts and every ten minutes and pushes as the files
//! change. New with the Rust version: the tmux version had only the rules'
//! repository (`claude.json`, src/setup/claude_md.rs), and a folder linked
//! from a dotfiles clone, where what Settings wrote sat uncommitted.
//!
//! `paths::config_dir()` is the one place that decides where your files
//! are: `dir()` here, once the clone is there, else `~/.config/switchboard`
//! as before. It is asked on every read, so it costs a few `stat`s: what
//! files.json and the setting say is kept until either file changes. Some
//! files never leave the computer, read and written in
//! `~/.config/switchboard` whatever this says (`LOCAL`, through
//! `paths::local_config_dir()`): files.json itself and the setting that
//! turns this on, which say where the rest are; what decides the code that
//! runs here with no one asked, so a pull never does (`trusted-dirs`,
//! `repos.json`, whose commands a landing runs, and `sidebar-plugins/`,
//! whose programs the server runs); `slack-relay.json` and
//! `discord-relay.json`, which name the one
//! person the relay answers to; `secrets.json`, `linear.json` (it may hold
//! your Linear key) and `leak-check.txt`, which hold what must not be
//! published; the files Claude's skills read by their path in
//! `~/.config/switchboard` (`oncall.json`, `oncall-rules.md`,
//! `slack-updates.json`); `autopilot-claim.json`, the ID autopilot claims
//! GitHub issues with, which is each computer's own; and the `.bak` copies. What a pull does
//! bring is your settings and where your rules come from (`claude.json`), so
//! the repository named is one you trust with those.
//!
//! The work is `follow`'s, a task of the server's own: it looks every two
//! seconds at what the folder holds (names, sizes and times, no process),
//! and only when that changed, or a pull is due, runs git (`sync`): the
//! changes committed ("Switchboard: settings.json"), what the remote has
//! fetched and taken fast-forward only, then the commits pushed, with git's
//! own hooks. A push that fails is said once and tried again with the next
//! change or pull; a remote that can't be taken fast-forward stops the
//! commits being pushed and says so rather than merging, until a pull
//! works again. Before a push, `leak-check` reads what it would publish,
//! and anything it finds is asked about first (src/files/serve.rs), since
//! the repository may be public. Nothing prompts: git's and ssh's askpass
//! are off and gh's sign-in is among the credential helpers on GitHub, as
//! for your rules' repository.
//!
//! The first time, the repository is cloned beside its place and the files
//! you have in `~/.config/switchboard` that it lacks are copied into
//! `<dir>` before the clone is moved into place, so nothing reads a
//! half-made folder; where both have a file the repository's wins, and
//! yours stays where it was. The clone checks out no symbolic links
//! (`core.symlinks` off), so a link in the repository can't lead a write
//! out of it. A clone of another repository, from before
//! files.json changed, is moved aside rather than pulled. Whenever where
//! your files are changes (the clone made, the setting turned off), or the
//! clone's claude.json does (a pull bringing another computer's), setup's
//! `claude-md` part runs again where it is set up (as a server starts too,
//! when they moved while none ran: `files-dir` in the state directory says
//! where they were), since
//! `~/.claude/CLAUDE.md` imports your rules by their path.

pub mod serve;

use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

use crate::mux::server::Event;
use crate::{paths, settings, util, worktree};

/// The setting that turns this on, kept in this computer's own
/// settings.json (`settings::LOCAL`).
pub const SETTING: &str = "files_from_repo";

/// The files of yours that stay in `~/.config/switchboard`.
pub const LOCAL: &[&str] = &[
    "files.json",
    "secrets.json",
    "trusted-dirs",
    "leak-check.txt",
    "repos.json",
    "slack-relay.json",
    "discord-relay.json",
    "sidebar-plugins",
    "linear.json",
    "oncall.json",
    "oncall-rules.md",
    "slack-updates.json",
    "autopilot-claim.json",
];

/// git's setting for the clone that checks a link the repository holds out
/// as a plain file naming its target, never as a link: Settings writes
/// through a link to the file it names, so one pulled in could point a
/// write at a file that stays on this computer (repos.json, trusted-dirs).
const NO_LINKS: &str = "core.symlinks=false";

/// How often the folder is looked at for changes.
const LOOK: Duration = Duration::from_secs(2);
/// How often the repository is pulled.
const PULL_EVERY: Duration = Duration::from_secs(600);
/// How long after a clone that failed before the next try.
const CLONE_AGAIN: Duration = Duration::from_secs(300);
/// What `source` the request audit counts its git requests under.
const SOURCE: &str = "your files' repository";

/// Whether a file of yours stays on this computer: one of `LOCAL`, a
/// `.bak` copy, or a file half written (`util::write_atomic`'s `.tmp`).
pub fn is_local(name: &str) -> bool {
    LOCAL.contains(&name) || name.ends_with(".bak") || name.ends_with(".tmp")
}

/// Where a file of yours is: `~/.config/switchboard` for one that stays
/// local, else where your files are now.
pub fn file(name: &str) -> PathBuf {
    if is_local(name) {
        paths::local_config_dir().join(name)
    } else {
        paths::config_dir().join(name)
    }
}

/// A file of yours as a message names it: `~/.config/switchboard/<name>`,
/// or where it is in the clone while your files come from a repository.
pub fn shown(name: &str) -> String {
    match dir() {
        Some(dir) if !is_local(name) => paths::tilde(dir.join(name)),
        _ => format!("~/.config/switchboard/{name}"),
    }
}

/// What files.json names: the repository, and the folder in it that holds
/// your files (empty for its root).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Named {
    pub repo: String,
    pub dir: PathBuf,
}

/// `~/.config/switchboard/files.json`.
pub fn config_file() -> PathBuf {
    paths::local_config_dir().join("files.json")
}

/// The clone of your files' repository.
pub fn clone_dir() -> PathBuf {
    paths::home_join(".local/share/switchboard/files")
}

/// What a files.json names, or what is wrong with it, in words. `dir` is
/// `switchboard` when not set, and `.` the repository's root; it stays
/// inside the repository.
pub fn parse(config: &Value) -> Result<Named, String> {
    let text = |key: &str| config.get(key).and_then(Value::as_str).map(str::trim);
    let repo = text("repo")
        .filter(|r| !r.is_empty())
        .ok_or("files.json names no repository: set \"repo\" to its URL, as git clone takes it")?;
    let dir = Path::new(
        text("dir")
            .filter(|d| !d.is_empty())
            .unwrap_or("switchboard"),
    );
    let mut inside = PathBuf::new();
    for part in dir.components() {
        match part {
            Component::CurDir => {}
            Component::Normal(name) if name != ".git" => inside.push(name),
            _ => {
                return Err(
                    "files.json's \"dir\" must be a folder inside the repository (switchboard, or . for its root)"
                        .into(),
                );
            }
        }
    }
    Ok(Named {
        repo: repo.to_string(),
        dir: inside,
    })
}

/// Whether this computer's settings.json turns the setting on. Read from
/// the file itself, since the settings are read from where this says.
fn turned_on() -> bool {
    util::read_json_object(settings::local_settings_file()).get(SETTING) == Some(&Value::Bool(true))
}

type Seen = Option<(SystemTime, u64)>;

fn seen(file: &Path) -> Seen {
    let meta = fs::metadata(file).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// What files.json names while the setting is on, else `None`: read again
/// only when files.json or this computer's settings.json changed. Never in
/// the library's own unit tests, whose home is yours.
pub fn named() -> Option<Named> {
    type Known = (PathBuf, Seen, Seen, Option<Named>);
    static KNOWN: Mutex<Option<Known>> = Mutex::new(None);
    if cfg!(test) {
        return None;
    }
    let file = config_file();
    let config = seen(&file)?;
    let settings = seen(&settings::local_settings_file());
    let mut known = KNOWN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((f, c, s, named)) = known.as_ref()
        && *f == file
        && *c == Some(config)
        && *s == settings
    {
        return named.clone();
    }
    let named = turned_on()
        .then(|| util::read_json(&file))
        .flatten()
        .and_then(|config| parse(&config).ok());
    *known = Some((file, Some(config), settings, named.clone()));
    named
}

/// What is wrong with files.json while the setting is on, for saying once.
fn problem() -> Option<String> {
    if cfg!(test) || !turned_on() || !config_file().exists() {
        return None;
    }
    match util::read_json(config_file()) {
        Some(config) => parse(&config).err(),
        None => Some("files.json can't be read as JSON".into()),
    }
}

/// Where your files are while they come from a repository: the folder in
/// the clone, once the clone is there with it.
pub fn dir() -> Option<PathBuf> {
    let named = named()?;
    let clone = clone_dir();
    let dir = clone.join(&named.dir);
    (clone.join(".git").exists() && dir.is_dir()).then_some(dir)
}

/// The repository your files come from now, when they do.
pub fn repo() -> Option<String> {
    dir().and(named()).map(|n| n.repo)
}

// ── git ─────────────────────────────────────────────────────────────────

/// git in the clone, asking for nothing: its prompt and askpass off, ssh's
/// too, its input closed.
fn git(clone: &Path, helper: Option<&str>) -> Command {
    let mut cmd = Command::new("git");
    if let Some(helper) = helper {
        cmd.arg("-c").arg(format!("credential.helper={helper}"));
    }
    cmd.arg("-C")
        .arg(clone)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "false")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .stdin(Stdio::null());
    cmd
}

/// What a git command printed, or the last line of what it said went wrong.
fn run(cmd: &mut Command) -> Result<String, String> {
    let out = cmd.output().map_err(|e| format!("git didn't run: {e}"))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string());
    }
    let said = String::from_utf8_lossy(&out.stderr);
    Err(said
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("git failed")
        .to_string())
}

/// The folder and what in it is never committed, as git's pathspecs from
/// the clone's top.
fn pathspecs(dir: &Path) -> Vec<String> {
    let root = dir.as_os_str().is_empty();
    let under = |name: &str| {
        if root {
            name.to_string()
        } else {
            format!("{}/{name}", dir.display())
        }
    };
    let mut specs = vec![if root {
        ".".to_string()
    } else {
        dir.display().to_string()
    }];
    specs.extend(
        LOCAL
            .iter()
            .map(|name| format!(":(exclude){}", under(name))),
    );
    for kept in ["bak", "tmp"] {
        specs.push(format!(":(exclude,glob){}", under(&format!("**/*.{kept}"))));
    }
    specs
}

/// Commits what changed in the folder, giving the commit's subject, or
/// `None` when nothing did.
pub fn commit(clone: &Path, dir: &Path) -> Result<Option<String>, String> {
    let specs = pathspecs(dir);
    run(git(clone, None).args(["add", "-A", "--"]).args(&specs))?;
    let changed = run(git(clone, None)
        .args(["diff", "--cached", "--name-only", "--no-renames", "--"])
        .args(&specs))?;
    let names: Vec<&str> = changed
        .lines()
        .map(|line| {
            Path::new(line)
                .strip_prefix(dir)
                .ok()
                .and_then(Path::to_str)
                .unwrap_or(line)
        })
        .collect();
    let subject = match names.as_slice() {
        [] => return Ok(None),
        [one] => format!("Switchboard: {one}"),
        [one, two] => format!("Switchboard: {one}, {two}"),
        [one, two, rest @ ..] => format!("Switchboard: {one}, {two} and {} more", rest.len()),
    };
    run(git(clone, None)
        .args(["commit", "--quiet", "-m", &subject, "--"])
        .args(&specs))?;
    Ok(Some(subject))
}

/// The commits not on the remote yet, as a range `git log` takes, and the
/// commit at their tip; `None` when there are none.
fn unpushed(clone: &Path) -> Option<(String, String)> {
    let tip = run(git(clone, None).args(["rev-parse", "--verify", "--quiet", "HEAD"])).ok()?;
    let upstream = run(git(clone, None).args(["rev-parse", "--verify", "--quiet", "@{u}"])).is_ok();
    if !upstream {
        return Some(("HEAD".into(), tip));
    }
    let count = run(git(clone, None).args(["rev-list", "--count", "@{u}..HEAD"])).ok()?;
    (count != "0").then(|| ("@{u}..HEAD".into(), tip))
}

/// What the remote has, fetched, or why it couldn't be reached.
fn fetch(clone: &Path, helper: Option<&str>) -> Result<(), String> {
    worktree::count_fetch(clone, "origin", SOURCE);
    run(git(clone, helper).args(["fetch", "--quiet", "origin"])).map(|_| ())
}

/// What was fetched taken fast-forward only; fine too with no branch on the
/// remote to take.
fn fast_forward(clone: &Path) -> bool {
    run(git(clone, None).args(["rev-parse", "--verify", "--quiet", "@{u}"])).is_err()
        || run(git(clone, None).args(["merge", "--quiet", "--ff-only", "@{u}"])).is_ok()
}

fn push(clone: &Path, helper: Option<&str>) -> Result<(), String> {
    worktree::count_fetch(clone, "origin", SOURCE);
    run(git(clone, helper).args(["push", "--quiet", "-u", "origin", "HEAD"])).map(|_| ())
}

fn origin_of(clone: &Path) -> String {
    run(git(clone, None).args(["remote", "get-url", "origin"])).unwrap_or_default()
}

// ── The clone ───────────────────────────────────────────────────────────

/// Your files in `~/.config/switchboard` that `dest` lacks, copied into it
/// (what a link names, as a file), but those that stay local.
fn seed(local: &Path, dest: &Path, top: bool, depth: usize) -> std::io::Result<()> {
    let Ok(entries) = fs::read_dir(local) else {
        return Ok(());
    };
    fs::create_dir_all(dest)?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let to = dest.join(&name);
        if (top && is_local(&name.to_string_lossy())) || name == ".git" {
            continue;
        }
        // A link is followed; one naming nothing is left.
        let Ok(meta) = fs::metadata(entry.path()) else {
            continue;
        };
        if meta.is_dir() {
            if depth < 8 {
                seed(&entry.path(), &to, false, depth + 1)?;
            }
        } else if fs::symlink_metadata(&to).is_err() {
            fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

/// The clone made, or its folder, when either is missing; what to say of
/// it. A clone of another repository is moved aside first.
fn set_up(
    named: &Named,
    clone: &Path,
    local: &Path,
    helper: impl Fn() -> Option<String>,
) -> Result<Vec<String>, String> {
    let mut said = Vec::new();
    let io = |e: std::io::Error| e.to_string();
    if clone.join(".git").exists() && origin_of(clone) != named.repo {
        let aside = clone.with_file_name(format!("files.bak.{}", crate::setup::stamp()));
        fs::rename(clone, &aside).map_err(io)?;
        said.push(format!(
            "The clone of the repository your files came from before is kept in {}",
            paths::tilde(&aside)
        ));
    }
    if clone.join(".git").exists() {
        // A clone made another way (your rules' own, before this one
        // served both) checks out no links from here on either.
        let _ = run(git(clone, None).args(["config", "core.symlinks", "false"]));
    }
    if !clone.join(".git").exists() {
        let partial = clone.with_file_name("files.partial");
        let _ = fs::remove_dir_all(&partial);
        if let Some(parent) = clone.parent() {
            util::make_dirs(parent).map_err(io)?;
        }
        let cloned = run(git(parent_of(clone), helper().as_deref())
            .args(["clone", "--quiet", "-c", NO_LINKS, "--", &named.repo])
            .arg(&partial));
        if let Err(why) = cloned {
            let _ = fs::remove_dir_all(&partial);
            return Err(format!("couldn't clone {}: {why}", named.repo));
        }
        worktree::count_fetch(&partial, "origin", SOURCE);
        let made = seed(local, &partial.join(&named.dir), true, 0)
            .and_then(|()| fs::rename(&partial, clone));
        if let Err(e) = made {
            let _ = fs::remove_dir_all(&partial);
            return Err(e.to_string());
        }
    } else if !clone.join(&named.dir).is_dir() {
        // A folder the repository lacks, made whole beside its place.
        let dir = clone.join(&named.dir);
        let partial = dir.with_file_name(format!(
            ".{}.partial",
            dir.file_name().unwrap_or_default().to_string_lossy()
        ));
        let _ = fs::remove_dir_all(&partial);
        if let Some(parent) = dir.parent() {
            fs::create_dir_all(parent).map_err(io)?;
        }
        seed(local, &partial, true, 0)
            .and_then(|()| fs::rename(&partial, &dir))
            .map_err(io)?;
    } else {
        return Ok(said);
    }
    said.push(format!(
        "Your Switchboard files now come from {} ({})",
        named.repo,
        paths::tilde(clone.join(&named.dir))
    ));
    Ok(said)
}

fn parent_of(path: &Path) -> &Path {
    path.parent().unwrap_or(Path::new("/"))
}

/// What the folder holds: every file's name, size and time, but those never
/// committed, so a change is seen with no process started.
pub fn print(dir: &Path) -> Vec<(PathBuf, u64, Option<SystemTime>)> {
    fn walk(
        at: &Path,
        top: &Path,
        depth: usize,
        out: &mut Vec<(PathBuf, u64, Option<SystemTime>)>,
    ) {
        let Ok(entries) = fs::read_dir(at) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name == ".git" || is_local(&name.to_string_lossy()) {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            let path = entry.path();
            if meta.is_dir() {
                if depth < 8 {
                    walk(&path, top, depth + 1, out);
                }
            } else {
                let rel = path.strip_prefix(top).unwrap_or(&path).to_path_buf();
                out.push((rel, meta.len(), meta.modified().ok()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, 0, &mut out);
    out.sort();
    out
}

// ── The sync ────────────────────────────────────────────────────────────

/// What a sync has to tell the server.
#[derive(Debug)]
pub enum FilesEvent {
    /// Something to say.
    Said(String),
    /// leak-check found these in the commits whose tip is `tip`, which
    /// wait on an answer.
    Found { tip: String, hits: Vec<String> },
}

/// Your answers to "push anyway?", by the tip asked about.
static ANSWERS: Mutex<Option<HashMap<String, bool>>> = Mutex::new(None);
/// A sync asked for now (M-p's "Sync your Switchboard files now", or an
/// answer given).
static NOW: AtomicBool = AtomicBool::new(false);
/// One asked for by hand, which says how it went whatever happened.
static BY_HAND: AtomicBool = AtomicBool::new(false);

/// Records the answer to a `Found`, and has the next look act on it.
pub fn answer(tip: &str, push: bool) {
    ANSWERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(tip.to_string(), push);
    NOW.store(true, Ordering::SeqCst);
}

fn answered(tip: &str) -> Option<bool> {
    ANSWERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|answers| answers.get(tip).copied())
}

/// Has the next look pull and push now, and say how it went.
pub fn sync_now() {
    BY_HAND.store(true, Ordering::SeqCst);
    NOW.store(true, Ordering::SeqCst);
}

/// What `follow` keeps between looks.
#[derive(Default)]
struct Following {
    /// The repository followed, so a change of it starts afresh.
    repo: Option<Named>,
    pulled: Option<Instant>,
    print: Option<Vec<(PathBuf, u64, Option<SystemTime>)>>,
    clone_failed: Option<Instant>,
    /// Said already, so not said again until it has worked once.
    push_failed: bool,
    diverged: bool,
    commit_failed: bool,
    /// The tip leak-check's finds were last asked about.
    asked: Option<String>,
    problem: Option<String>,
}

/// How a sync went.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Synced {
    pub committed: Option<String>,
    pub commit_error: Option<String>,
    /// Why the remote couldn't be fetched, so nothing of its was taken.
    pub fetch_error: Option<String>,
    /// The remote and the clone both have commits the other lacks.
    pub diverged: bool,
    pub pushed: bool,
    pub push_error: Option<String>,
    /// leak-check's finds in what waits to be pushed, and its tip.
    pub found: Option<(String, Vec<String>)>,
    /// Commits wait that you said not to push.
    pub held: bool,
}

/// One sync of the clone: the folder's changes committed, the remote
/// fetched when `pull` or when there is something to push, taken
/// fast-forward only, and what is unpushed pushed, unless leak-check finds
/// something in it that `approved` doesn't say to push anyway.
pub fn sync(
    clone: &Path,
    dir: &Path,
    helper: impl Fn() -> Option<String>,
    pull: bool,
    approved: impl Fn(&str) -> Option<bool>,
) -> Synced {
    let mut synced = Synced::default();
    match commit(clone, dir) {
        Ok(subject) => synced.committed = subject,
        Err(why) => synced.commit_error = Some(why),
    }
    let waiting = unpushed(clone);
    if !pull && waiting.is_none() {
        return synced;
    }
    // Only now, with the network about to be asked: resolving it may start
    // a process (gh's), which a look that finds nothing to do never does.
    let helper = helper();
    let helper = helper.as_deref();
    match fetch(clone, helper) {
        Ok(()) if !fast_forward(clone) => {
            synced.diverged = true;
            return synced;
        }
        Ok(()) => {}
        Err(why) => synced.fetch_error = Some(why),
    }
    // A fast-forward may have brought what was waiting.
    let Some((range, tip)) = unpushed(clone) else {
        return synced;
    };
    match approved(&tip) {
        Some(true) => {}
        Some(false) => {
            synced.held = true;
            return synced;
        }
        None => match crate::cmd::leak_check::look(Some(clone), &range) {
            Ok(looked) if looked.kept.is_empty() => {}
            Ok(looked) => {
                let hits = looked.kept.into_iter().map(|hit| hit.shown).collect();
                synced.found = Some((tip, hits));
                return synced;
            }
            Err(why) => {
                synced.push_error = Some(format!("leak-check couldn't read them ({why})"));
                return synced;
            }
        },
    }
    match push(clone, helper) {
        Ok(()) => synced.pushed = true,
        Err(why) => synced.push_error = Some(why),
    }
    synced
}

/// One look: the clone made when it is missing, and a sync when the folder
/// changed, a pull is due or one was asked for. Gives what to tell the
/// server.
fn look(state: &mut Following) -> Vec<FilesEvent> {
    let mut events = Vec::new();
    let now = NOW.swap(false, Ordering::SeqCst);
    let by_hand = BY_HAND.swap(false, Ordering::SeqCst);
    let Some(named) = named() else {
        let problem = problem();
        if let Some(why) = problem
            .clone()
            .filter(|_| state.problem != problem || by_hand)
        {
            events.push(FilesEvent::Said(format!(
                "Your Switchboard files stay on this computer: {why}"
            )));
        } else if by_hand && problem.is_none() {
            events.push(FilesEvent::Said("Your Switchboard files aren't kept in a repository: turn on Settings' \"Your files from a git repository\" and name the repository under Your files (files.json)".into()));
        }
        *state = Following {
            problem,
            ..Following::default()
        };
        return events;
    };
    if state.repo.as_ref() != Some(&named) {
        *state = Following {
            repo: Some(named.clone()),
            ..Following::default()
        };
    }
    let clone = clone_dir();
    let dir = clone.join(&named.dir);
    let helper = || crate::setup::claude_md::gh_helper(&named.repo);
    let mut fresh = false;
    if !clone.join(".git").exists() || !dir.is_dir() || state.pulled.is_none() {
        if !now
            && state
                .clone_failed
                .is_some_and(|at| at.elapsed() < CLONE_AGAIN)
        {
            return events;
        }
        match set_up(&named, &clone, &paths::local_config_dir(), helper) {
            Ok(said) => {
                fresh = !said.is_empty();
                events.extend(said.into_iter().map(FilesEvent::Said));
                state.clone_failed = None;
            }
            Err(why) => {
                if state.clone_failed.is_none() || by_hand {
                    events.push(FilesEvent::Said(format!(
                        "Your Switchboard files stay on this computer for now: {why}"
                    )));
                }
                state.clone_failed = Some(Instant::now());
                return events;
            }
        }
    }
    let due = state.pulled.is_none_or(|at| at.elapsed() >= PULL_EVERY);
    let printed = print(&dir);
    if !(due || now || fresh || state.print.as_ref() != Some(&printed)) {
        return events;
    }
    let pull = due || now;
    if pull {
        state.pulled = Some(Instant::now());
    }
    let synced = sync(&clone, &named.dir, helper, pull, answered);
    // As it was before the sync's commit: a file saved while the fetch or
    // the push waited is then seen by the next look, and so is what a pull
    // brought, which costs that look a commit with nothing in it.
    state.print = Some(printed);
    let at = paths::tilde(&clone);
    match &synced.commit_error {
        Some(why) if !state.commit_failed || by_hand => events.push(FilesEvent::Said(format!(
            "Couldn't commit your Switchboard files in {at}: {why}"
        ))),
        _ => {}
    }
    state.commit_failed = synced.commit_error.is_some();
    if synced.diverged {
        if !state.diverged || by_hand {
            events.push(FilesEvent::Said(format!(
                "Your Switchboard files changed here and in their repository, so they aren't pushed: bring {at} up to date by hand (git pull --rebase there), and the next pull goes on"
            )));
        }
        state.diverged = true;
        return events;
    }
    state.diverged = false;
    if let Some((tip, hits)) = synced.found {
        if state.asked.as_ref() != Some(&tip) || by_hand {
            state.asked = Some(tip.clone());
            events.push(FilesEvent::Found { tip, hits });
        }
        return events;
    }
    match &synced.push_error {
        Some(why) => {
            if !state.push_failed || by_hand {
                events.push(FilesEvent::Said(format!(
                    "Couldn't push your Switchboard files ({why}); they are committed in {at} and pushed with the next change"
                )));
            }
            state.push_failed = true;
        }
        // A pull that couldn't reach the remote is said only when asked
        // for by hand: a computer off the network is nothing to announce
        // every ten minutes, and a push it stops is said above.
        None if synced.fetch_error.is_some() => {
            if let (true, Some(why)) = (by_hand, &synced.fetch_error) {
                events.push(FilesEvent::Said(format!(
                    "Couldn't reach your Switchboard files' repository ({why}); the next pull tries again"
                )));
            }
        }
        None => {
            state.push_failed = false;
            if by_hand {
                events.push(FilesEvent::Said(
                    if synced.pushed {
                        "Your Switchboard files are pushed"
                    } else if synced.held {
                        "Your Switchboard files are up to date, but for the commits you said not to push"
                    } else {
                        "Your Switchboard files are up to date"
                    }
                    .into(),
                ));
            }
        }
    }
    events
}

/// The server's task for your files: a look every `LOOK`, off the server's
/// own task, telling it what to say or ask.
pub async fn follow(tx: UnboundedSender<Event>) {
    let mut state = Following::default();
    let mut every = tokio::time::interval(LOOK);
    let mut rules = rules_from();
    // Where your files were when a server last looked, kept in the state
    // directory: moved while none ran (the setting turned off by hand, say),
    // the rules are imported again now.
    let kept = paths::state_dir().join("files-dir");
    let before = util::read_text(&kept);
    let moved = |before: &Option<String>, now: &Path| match before {
        Some(was) => Path::new(was.trim()) != now,
        None => now != paths::local_config_dir(),
    };
    // When an import failed (a rules repository that couldn't be cloned),
    // the time to try it again.
    let mut again: Option<Instant> = None;
    if moved(&before, &rules.0) && !imported().await {
        again = Some(Instant::now() + CLONE_AGAIN);
    }
    if before.as_deref().map(str::trim) != rules.0.to_str() {
        let _ = util::write_atomic(&kept, rules.0.to_string_lossy().as_bytes());
    }
    loop {
        every.tick().await;
        let now = rules_from();
        if now != rules {
            if now.0 != rules.0 {
                let _ = util::write_atomic(&kept, now.0.to_string_lossy().as_bytes());
            }
            rules = now;
            again = Some(Instant::now());
        }
        if again.is_some_and(|at| at <= Instant::now()) {
            again = (!imported().await).then(|| Instant::now() + CLONE_AGAIN);
        }
        // Nothing named and nothing asked: no thread is started for it.
        if named().is_none()
            && !NOW.load(Ordering::SeqCst)
            && state.repo.is_none()
            && problem() == state.problem
        {
            continue;
        }
        let looked = tokio::task::spawn_blocking(move || {
            let events = look(&mut state);
            (state, events)
        })
        .await;
        let Ok((kept, events)) = looked else {
            state = Following::default();
            continue;
        };
        state = kept;
        for event in events {
            if tx.send(Event::Files(event)).is_err() {
                return;
            }
        }
    }
}

/// What decides where your rules are imported from, as far as your files'
/// repository can change it: where your files are, and, while they come
/// from the clone, its claude.json as it is (a pull may bring another).
fn rules_from() -> (PathBuf, Seen) {
    let config = paths::config_dir();
    let claude = dir().and_then(|dir| seen(&dir.join("claude.json")));
    (config, claude)
}

/// Your `~/.claude/CLAUDE.md` written again after your files moved, where
/// setup wrote it: it imports your rules (`CLAUDE.md` among your files,
/// unless claude.json names them elsewhere) by their path. Not by a server
/// with a state directory of its own, which leaves `~/.claude` as it is.
/// Whether it worked, or there was nothing to do.
fn import_rules_again() -> bool {
    if crate::setup::state_of_its_own() || !crate::setup::claude_md::set_up() {
        return true;
    }
    match crate::setup::run_claude_md() {
        Ok(_) => true,
        Err(why) => {
            eprintln!("switchboard server: your CLAUDE.md after your files moved: {why}");
            false
        }
    }
}

/// `import_rules_again` off the task; a run that failed is tried again
/// after `CLONE_AGAIN`.
async fn imported() -> bool {
    tokio::task::spawn_blocking(import_rules_again)
        .await
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// An address leak-check finds, written in two pieces so it finds none
    /// in this file.
    const ADDRESS: &str = concat!("me@", "corp.test");

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sb-files-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn git_in(dir: &Path, args: &[&str]) -> String {
        run(git(dir, None).args(args)).unwrap_or_else(|e| panic!("git {args:?}: {e}"))
    }

    /// A bare remote and a clone of it with an identity to commit under.
    fn remote_and_clone(name: &str) -> (PathBuf, PathBuf) {
        let top = temp(name);
        let remote = top.join("remote.git");
        fs::create_dir_all(&remote).unwrap();
        git_in(&remote, &["init", "--quiet", "--bare", "-b", "main"]);
        let clone = top.join("clone");
        git_in(
            &top,
            &[
                "clone",
                "--quiet",
                remote.to_str().unwrap(),
                clone.to_str().unwrap(),
            ],
        );
        for (key, value) in [
            ("user.name", "Someone"),
            ("user.email", "someone@example.com"),
        ] {
            git_in(&clone, &["config", key, value]);
        }
        git_in(&clone, &["symbolic-ref", "HEAD", "refs/heads/main"]);
        (remote, clone)
    }

    #[test]
    fn files_json_names_a_repository_and_a_folder_inside_it() {
        let named = parse(&json!({"repo": " git@example.com:me/dotfiles.git "})).unwrap();
        assert_eq!(named.repo, "git@example.com:me/dotfiles.git");
        assert_eq!(named.dir, Path::new("switchboard"));
        let root = parse(&json!({"repo": "r", "dir": "."})).unwrap();
        assert_eq!(root.dir, Path::new(""));
        let nested = parse(&json!({"repo": "r", "dir": "./config/switchboard/"})).unwrap();
        assert_eq!(nested.dir, Path::new("config/switchboard"));
        for outside in ["../elsewhere", "/etc", "a/../../b", ".git", "a/.git/hooks"] {
            assert!(
                parse(&json!({"repo": "r", "dir": outside})).is_err(),
                "{outside}"
            );
        }
        assert!(parse(&json!({"dir": "switchboard"})).is_err());
        assert!(parse(&json!({"repo": "  "})).is_err());
    }

    #[test]
    fn the_files_that_stay_local_are_never_read_from_the_repository() {
        for name in [
            "files.json",
            "secrets.json",
            "trusted-dirs",
            "leak-check.txt",
            "repos.json",
            "slack-relay.json",
            "discord-relay.json",
            "sidebar-plugins",
            "linear.json",
            "oncall.json",
            "oncall-rules.md",
            "slack-updates.json",
            "autopilot-claim.json",
            "settings.json.bak",
        ] {
            assert!(is_local(name), "{name}");
            assert_eq!(file(name), paths::local_config_dir().join(name));
        }
        assert!(!is_local("settings.json"));
        assert!(!is_local("keys.json"));
    }

    #[test]
    fn a_change_is_committed_under_its_files_names_and_pushed_but_never_a_local_file() {
        let (remote, clone) = remote_and_clone("commit");
        let dir = clone.join("switchboard");
        fs::create_dir_all(dir.join("sidebar-plugins")).unwrap();
        fs::write(dir.join("settings.json"), "{}\n").unwrap();
        fs::write(dir.join("secrets.json"), "{\"token\": \"x\"}\n").unwrap();
        fs::write(dir.join("trusted-dirs"), "~/code\n").unwrap();
        fs::write(dir.join("repos.json"), "{}\n").unwrap();
        fs::write(dir.join("sidebar-plugins/run-me"), "#!/bin/sh\n").unwrap();
        fs::write(dir.join("settings.json.bak"), "{}\n").unwrap();
        fs::write(dir.join("sidebar-plugins/mine.json.bak"), "{}\n").unwrap();
        fs::write(clone.join("README.md"), "not Switchboard's\n").unwrap();
        let synced = sync(&clone, Path::new("switchboard"), || None, false, |_| None);
        assert_eq!(
            synced.committed.as_deref(),
            Some("Switchboard: settings.json")
        );
        assert!(synced.pushed, "{synced:?}");
        assert_eq!(
            git_in(&remote, &["ls-tree", "-r", "--name-only", "main"]),
            "switchboard/settings.json"
        );
        // Nothing changed: nothing committed, and no push.
        assert_eq!(
            sync(&clone, Path::new("switchboard"), || None, false, |_| None),
            Synced::default()
        );
        fs::write(dir.join("keys.json"), "{}\n").unwrap();
        fs::write(dir.join("browser.json"), "{}\n").unwrap();
        fs::remove_file(dir.join("settings.json")).unwrap();
        let synced = sync(&clone, Path::new("switchboard"), || None, false, |_| None);
        assert_eq!(
            synced.committed.as_deref(),
            Some("Switchboard: browser.json, keys.json and 1 more")
        );
        assert_eq!(
            git_in(&remote, &["ls-tree", "-r", "--name-only", "main"]),
            "switchboard/browser.json\nswitchboard/keys.json"
        );
    }

    #[test]
    fn what_the_remote_has_is_taken_fast_forward_and_one_that_diverged_is_left_alone() {
        let (remote, clone) = remote_and_clone("pull");
        let dir = clone.join("switchboard");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("settings.json"), "{}\n").unwrap();
        assert!(sync(&clone, Path::new("switchboard"), || None, false, |_| None).pushed);
        // Another computer pushes a change.
        let other = clone.with_file_name("other");
        git_in(
            parent_of(&clone),
            &[
                "clone",
                "--quiet",
                remote.to_str().unwrap(),
                other.to_str().unwrap(),
            ],
        );
        for (key, value) in [
            ("user.name", "Someone"),
            ("user.email", "someone@example.com"),
        ] {
            git_in(&other, &["config", key, value]);
        }
        fs::write(other.join("switchboard/settings.json"), "{\"a\": 1}\n").unwrap();
        assert!(sync(&other, Path::new("switchboard"), || None, false, |_| None).pushed);
        // Not pulled without a pull being due and nothing to push.
        sync(&clone, Path::new("switchboard"), || None, false, |_| None);
        assert_eq!(
            fs::read_to_string(dir.join("settings.json")).unwrap(),
            "{}\n"
        );
        let synced = sync(&clone, Path::new("switchboard"), || None, true, |_| None);
        assert!(!synced.diverged && !synced.pushed, "{synced:?}");
        assert_eq!(
            fs::read_to_string(dir.join("settings.json")).unwrap(),
            "{\"a\": 1}\n"
        );
        // Both change the same file: nothing is merged, nothing pushed.
        fs::write(other.join("switchboard/settings.json"), "{\"a\": 2}\n").unwrap();
        assert!(sync(&other, Path::new("switchboard"), || None, false, |_| None).pushed);
        fs::write(dir.join("settings.json"), "{\"a\": 3}\n").unwrap();
        let synced = sync(&clone, Path::new("switchboard"), || None, false, |_| None);
        assert!(synced.diverged && !synced.pushed, "{synced:?}");
        assert_eq!(
            fs::read_to_string(dir.join("settings.json")).unwrap(),
            "{\"a\": 3}\n"
        );
        assert_eq!(
            git_in(&remote, &["rev-parse", "main"]),
            git_in(&other, &["rev-parse", "HEAD"])
        );
    }

    #[test]
    fn a_link_in_the_repository_is_checked_out_as_a_file_never_a_link() {
        let (remote, first) = remote_and_clone("links");
        fs::create_dir_all(first.join("switchboard")).unwrap();
        std::os::unix::fs::symlink(
            "../../elsewhere.json",
            first.join("switchboard/browser.json"),
        )
        .unwrap();
        git_in(&first, &["add", "-A"]);
        git_in(&first, &["commit", "--quiet", "-m", "a link"]);
        git_in(&first, &["push", "--quiet", "-u", "origin", "HEAD"]);
        let top = parent_of(&first).to_path_buf();
        let clone = top.join("share/files");
        let named = Named {
            repo: remote.to_str().unwrap().to_string(),
            dir: PathBuf::from("switchboard"),
        };
        set_up(&named, &clone, &top.join("local"), || None).unwrap();
        let pulled = fs::symlink_metadata(clone.join("switchboard/browser.json")).unwrap();
        assert!(pulled.is_file() && !pulled.file_type().is_symlink());
        // One pulled in later is a file too.
        std::os::unix::fs::symlink("../../other.json", first.join("switchboard/keys.json"))
            .unwrap();
        git_in(&first, &["add", "-A"]);
        git_in(&first, &["commit", "--quiet", "-m", "another link"]);
        git_in(&first, &["push", "--quiet"]);
        sync(&clone, Path::new("switchboard"), || None, true, |_| None);
        let pulled = fs::symlink_metadata(clone.join("switchboard/keys.json")).unwrap();
        assert!(pulled.is_file() && !pulled.file_type().is_symlink());
    }

    #[test]
    fn a_remote_that_cant_be_reached_is_an_error_not_up_to_date() {
        let (remote, clone) = remote_and_clone("unreachable");
        let dir = clone.join("switchboard");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("settings.json"), "{}\n").unwrap();
        assert!(sync(&clone, Path::new("switchboard"), || None, false, |_| None).pushed);
        fs::remove_dir_all(&remote).unwrap();
        let synced = sync(&clone, Path::new("switchboard"), || None, true, |_| None);
        assert!(synced.fetch_error.is_some() && !synced.pushed, "{synced:?}");
    }

    #[test]
    fn what_leak_check_finds_waits_on_an_answer_before_it_is_pushed() {
        let (remote, clone) = remote_and_clone("leak");
        let dir = clone.join("switchboard");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("browser.json"),
            format!("{{\"owner\": \"{ADDRESS}\"}}\n"),
        )
        .unwrap();
        let synced = sync(&clone, Path::new("switchboard"), || None, false, |_| None);
        let (tip, hits) = synced.found.expect("found");
        assert!(!synced.pushed);
        assert!(hits.iter().any(|h| h.contains(ADDRESS)), "{hits:?}");
        assert!(git_in(&remote, &["branch", "--list"]).is_empty());
        // Said no: kept back, and not looked at again.
        let held = sync(
            &clone,
            Path::new("switchboard"),
            || None,
            false,
            |_| Some(false),
        );
        assert!(held.held && !held.pushed);
        let pushed = sync(
            &clone,
            Path::new("switchboard"),
            || None,
            false,
            |t| (t == tip).then_some(true),
        );
        assert!(pushed.pushed, "{pushed:?}");
        assert_eq!(git_in(&remote, &["rev-parse", "main"]), tip);
    }

    #[test]
    fn the_first_clone_takes_in_the_files_you_have_that_the_repository_lacks() {
        let (remote, first) = remote_and_clone("seed");
        fs::create_dir_all(first.join("switchboard")).unwrap();
        fs::write(
            first.join("switchboard/settings.json"),
            "{\"theirs\": true}\n",
        )
        .unwrap();
        assert!(sync(&first, Path::new("switchboard"), || None, false, |_| None).pushed);
        let top = parent_of(&first).to_path_buf();
        let local = top.join("local");
        fs::create_dir_all(local.join("sidebar-plugins")).unwrap();
        fs::create_dir_all(local.join("themes")).unwrap();
        fs::write(local.join("settings.json"), "{\"mine\": true}\n").unwrap();
        fs::write(local.join("keys.json"), "{}\n").unwrap();
        fs::write(local.join("sidebar-plugins/one.sh"), "echo\n").unwrap();
        fs::write(local.join("themes/one.json"), "{}\n").unwrap();
        for name in LOCAL.iter().filter(|name| **name != "sidebar-plugins") {
            fs::write(local.join(name), "local\n").unwrap();
        }
        fs::write(local.join("settings.json.bak"), "{}\n").unwrap();
        let clone = top.join("share/files");
        let named = Named {
            repo: remote.to_str().unwrap().to_string(),
            dir: PathBuf::from("switchboard"),
        };
        let said = set_up(&named, &clone, &local, || None).unwrap();
        assert_eq!(said.len(), 1, "{said:?}");
        let dir = clone.join("switchboard");
        let names: Vec<PathBuf> = print(&dir).into_iter().map(|(name, _, _)| name).collect();
        assert_eq!(
            names,
            ["keys.json", "settings.json", "themes/one.json"].map(PathBuf::from)
        );
        // The repository's copy wins where both have the file.
        assert_eq!(
            fs::read_to_string(dir.join("settings.json")).unwrap(),
            "{\"theirs\": true}\n"
        );
        for name in LOCAL {
            assert!(!dir.join(name).exists(), "{name}");
        }
        // Set up already: nothing more to do or say.
        assert!(set_up(&named, &clone, &local, || None).unwrap().is_empty());
        // Another repository named: the clone is kept aside.
        let (other_remote, _) = remote_and_clone("seed-other");
        let moved = Named {
            repo: other_remote.to_str().unwrap().to_string(),
            dir: PathBuf::new(),
        };
        let said = set_up(&moved, &clone, &local, || None).unwrap();
        assert!(said[0].contains("files.bak."), "{said:?}");
        assert!(clone.join("keys.json").exists());
        assert_eq!(origin_of(&clone), moved.repo);
    }
}
