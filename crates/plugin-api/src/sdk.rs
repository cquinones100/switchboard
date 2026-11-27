//! The few helpers every plugin needs that the workspace needs too, taken
//! from `switchboard`'s src/util.rs and src/paths.rs, which give them at
//! the paths they had, so a plugin and the workspace find the home, write a
//! file whole and run a command one way.

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, anyhow};
use serde_json::Value;

use crate::{Action, Answer, Folders, Host};

/// A plugin answering for the workspace itself, with none running (its
/// binary alone): its settings are its defaults with what you saved over
/// them, its folders its own, nothing it reads is untrusted, what it copies
/// or counts is done, and whatever needs a workspace (a server, a PR, a
/// link, a picture, a checklist item, the pointer, a tab) answers
/// `Answer::None`. A plugin overrides only what differs, in `exception`.
pub trait AloneHost: Send + Sync {
    /// What a setting is while you haven't set it, `None` for one not listed.
    fn default_of(&self, key: &str) -> Option<Value>;
    /// What you saved for a setting, `None` with nothing saved or read.
    fn saved(&self, key: &str) -> Option<Value>;
    /// The plugin's own state and cache folders.
    fn own_folders(&self) -> Folders;
    /// The plugin's own answer where it differs from the default one.
    fn exception(&self, _action: &Action) -> Option<Answer> {
        None
    }
}

/// A setting alone: what was saved, when it is of the default's kind
/// (on/off for an on/off option, a word for a choice), else the default.
pub fn saved_over_default(saved: Option<Value>, default: Option<Value>) -> Option<Value> {
    match (saved, default) {
        (Some(v), Some(d)) if v.is_boolean() == d.is_boolean() => Some(v),
        (Some(v), None) => Some(v),
        (_, default) => default,
    }
}

impl<T: AloneHost> Host for T {
    fn ask(&self, action: Action) -> Answer {
        if let Some(answer) = self.exception(&action) {
            return answer;
        }
        match action {
            Action::Setting { key } => {
                match saved_over_default(self.saved(&key), self.default_of(&key)) {
                    Some(value) => Answer::Value { value },
                    None => Answer::None,
                }
            }
            Action::Folders => Answer::Folders {
                folders: self.own_folders(),
            },
            Action::InPane | Action::Untrusted { .. } => Answer::Flag { on: false },
            // The terminal's own clipboard (OSC 52) is the plugin's to write.
            Action::Copy { .. } | Action::Request { .. } | Action::Fetch { .. } => Answer::Done,
            _ => Answer::None,
        }
    }
}

/// `$HOME`, read every time, so a test or a try that changes it is followed.
pub fn home() -> PathBuf {
    match env::var_os("HOME") {
        Some(h) if !h.is_empty() => PathBuf::from(h),
        _ => env::home_dir().unwrap_or_else(|| PathBuf::from("/")),
    }
}

/// A path under the home directory: `home_join(".config/switchboard")`.
pub fn home_join(rel: impl AsRef<Path>) -> PathBuf {
    home().join(rel)
}

/// A path under the home directory written from `~` (`~/code/web`, or `~`
/// for the home itself), any other path as it is. The home's real path
/// counts too, since a worktree's root is read canonical: under a home in
/// `/tmp` on macOS it starts `/private/tmp`. The Node scripts wrote
/// `file.replace(os.homedir(), "~")`, which also matched the home inside
/// that real path (`/private~/code`).
pub fn tilde(path: impl AsRef<Path>) -> String {
    let path = path.as_ref();
    let home = home();
    if home.as_os_str().is_empty() || home == Path::new("/") {
        return path.display().to_string();
    }
    let rest = match path.strip_prefix(&home) {
        Ok(rest) => Some(rest),
        Err(_) => real_home(&home).and_then(|r| path.strip_prefix(r).ok()),
    };
    match rest {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// The home's real path, read once per home and kept, since `tilde` runs
/// on the drawing path, which never waits on the file system again.
fn real_home(home: &Path) -> Option<&'static Path> {
    static KNOWN: std::sync::Mutex<Vec<(PathBuf, Option<&'static Path>)>> =
        std::sync::Mutex::new(Vec::new());
    let mut known = KNOWN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, real)) = known.iter().find(|(h, _)| h == home) {
        return *real;
    }
    let real = home
        .canonicalize()
        .ok()
        .filter(|r| r != home)
        .map(|r| &*Box::leak(r.into_boxed_path()));
    known.push((home.to_path_buf(), real));
    real
}

/// Milliseconds since the epoch, as JavaScript's `Date.now()`.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Whole seconds since the epoch, as `Math.floor(Date.now() / 1000)`.
pub fn now_secs() -> i64 {
    now_ms().div_euclid(1000)
}

/// The command's path on `PATH`, if it is there and executable.
pub fn which(command: &str) -> Option<PathBuf> {
    if command.contains('/') {
        let p = PathBuf::from(command);
        return is_executable(&p).then_some(p);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(command))
        .find(|p| is_executable(p))
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// The whole output of a command that must succeed, its stdin closed.
pub fn output_of(cmd: &mut Command) -> Result<Output> {
    let program = cmd.get_program().to_string_lossy().into_owned();
    let output = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                anyhow!("{program}: command not found")
            } else {
                anyhow!("{program}: {e}")
            }
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let first = stderr
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim();
        let how = match output.status.code() {
            Some(code) => format!("exited {code}"),
            None => "was killed".to_string(),
        };
        return Err(anyhow!(
            "{program} {how}{}",
            if first.is_empty() {
                String::new()
            } else {
                format!(": {first}")
            }
        ));
    }
    Ok(output)
}

/// A file's text, or `None` when it can't be read.
pub fn read_text(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path).ok()
}

/// Writes a file whole: to `<file>.<pid>.tmp` beside it, then renamed into
/// place, so a reader never sees half of it. Its directory is made first,
/// as `make_dirs` makes it.
pub fn write_atomic(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> io::Result<()> {
    let path = path.as_ref();
    if let Some(dir) = path.parent() {
        make_dirs(dir)?;
    }
    let tmp = tmp_beside(path);
    fs::write(&tmp, contents)?;
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

fn tmp_beside(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(format!(".{}.tmp", std::process::id()));
    path.with_file_name(name)
}

/// Makes a directory and whatever is missing above it, each folder it makes
/// its owner's alone (0700), so the state, cache and config folders other
/// users could otherwise list and read are private from the start.
pub fn make_dirs(dir: impl AsRef<Path>) -> io::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

/// `JSON.stringify(value)`.
pub fn json_compact(value: &impl serde::Serialize) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".into())
}

/// Standard base64 with padding, for the clipboard sequence (OSC 52) and the
/// few places a protocol wants it.
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The path of the running binary, for starting another of its programs (a
/// pane's `switchboard diff`, a hook). Once `switchboard update` has put a
/// new binary in its place, Linux names the running one's file
/// "<path> (deleted)", since the file it started from is gone; the path
/// without that is where the new binary is, which is the one to start.
pub fn exe() -> io::Result<PathBuf> {
    let path = std::env::current_exe()?;
    let text = path.to_string_lossy();
    match text.strip_suffix(" (deleted)") {
        Some(kept) if !path.exists() => Ok(PathBuf::from(kept)),
        _ => Ok(path),
    }
}

/// Whether a remote's host is GitHub's: github.com and its ssh and www hosts,
/// or an ssh alias for it of the kind kept for a second account
/// (`github.com-work`, `github-personal`), which ~/.ssh/config maps to
/// github.com. A host with a dot after such a prefix is a server of its own
/// (GitHub Enterprise), whose API isn't api.github.com, so it isn't taken.
fn github_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    if matches!(
        host.as_str(),
        "github.com" | "ssh.github.com" | "www.github.com"
    ) {
        return true;
    }
    ["github.com-", "github-", "github_"].iter().any(|prefix| {
        host.strip_prefix(prefix)
            .is_some_and(|rest| !rest.contains('.'))
    })
}

/// The repository a remote URL names, in any form git takes for GitHub:
/// `git@github.com:owner/name.git`, `ssh://git@github.com/owner/name`,
/// `https://github.com/owner/name.git` (with a user or token before the
/// host), `git://...`: its owner and its name, each only of the characters
/// GitHub allows, which also keeps anything else out of a link or a REST
/// path they go into. `None` for a remote elsewhere.
pub fn github_remote(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    let (host, path) = if let Some((_, rest)) = url.split_once("://") {
        let (authority, path) = rest.split_once('/')?;
        let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        // A port (`ssh.github.com:443`), and IPv6's brackets, are not the host.
        let host = host.split(':').next().unwrap_or(host);
        (host, path)
    } else {
        // scp's form, `[user@]host:path`, which git reads only when the
        // colon comes before any slash.
        let (authority, path) = url.split_once(':')?;
        if authority.contains('/') {
            return None;
        }
        let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        (host, path)
    };
    if !github_host(host) {
        return None;
    }
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.split('/');
    let (Some(owner), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
        return None;
    };
    let part = |p: &str| {
        !p.is_empty()
            && p.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    };
    (part(owner) && part(name) && name != "." && name != "..")
        .then(|| (owner.to_string(), name.to_string()))
}

/// Makes each of `dirs` its owner's alone (0700), for a program as it
/// starts (the server, the diff alone): one that is missing is made so, as `make_dirs` makes it, since a
/// part may later make a folder inside with `create_dir_all`; one an earlier
/// build made open to other users is tightened. A link, or someone else's
/// folder, is left as it is.
pub fn make_private(dirs: &[PathBuf]) {
    // SAFETY: geteuid only reads the process's user id.
    let me = unsafe { libc::geteuid() };
    for dir in dirs {
        let Ok(meta) = dir.symlink_metadata() else {
            let _ = make_dirs(dir);
            continue;
        };
        if meta.is_dir() && meta.uid() == me && meta.mode() & 0o077 != 0 {
            let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
        }
    }
}

/// Whether a process is running: `kill(pid, 0)`, EPERM counting as running,
/// since it is there but someone else's.
pub fn is_alive(pid: i64) -> bool {
    if pid <= 0 || pid > i32::MAX as i64 {
        return false;
    }
    // SAFETY: signal 0 only checks that the process exists.
    let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
    r == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// A command's stdout, as `execFileSync(command, args, { encoding: "utf8" })`
/// gives it: an error, carrying the first line of stderr, when it can't start
/// or exits non-zero. Its stdin is closed.
pub fn run<I, S>(command: &str, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = Command::new(command);
    cmd.args(args);
    run_command(&mut cmd)
}

/// As `run`, for a command built by the caller (its directory, environment).
pub fn run_command(cmd: &mut Command) -> Result<String> {
    let output = output_of(cmd)?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// A command's stdout, or `None` when it fails in any way.
pub fn try_run<I, S>(command: &str, args: I) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run(command, args).ok()
}

/// A file read as JSON, or `None` when it is missing or isn't JSON.
pub fn read_json(path: impl AsRef<Path>) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// A file read as a JSON object, or an empty one, as the Node parts read
/// their state files: `try { JSON.parse(...) } catch { return {} }`.
pub fn read_json_object(path: impl AsRef<Path>) -> serde_json::Map<String, Value> {
    match read_json(path) {
        Some(Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    }
}

/// `JSON.stringify(value, null, 2)`.
pub fn json_pretty(value: &impl serde::Serialize) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| "null".into())
}

/// Claude Code's command: on the PATH, else where its installer puts it,
/// `~/.local/bin`, which the PATH of a program the server starts may leave out.
pub fn claude_command() -> Option<PathBuf> {
    which("claude").or_else(|| {
        let installed = home_join(".local/bin/claude");
        is_executable(&installed).then_some(installed)
    })
}

// ── git ─────────────────────────────────────────────────────────────────────

/// The flags that keep a git command that only looks from running anything
/// a repository supplies: no hook, no file-system monitor, and no optional
/// lock, so a look never holds the index lock a commit in a pane wants.
pub const GIT_SAFE: [&str; 5] = [
    "--no-optional-locks",
    "-c",
    "core.hooksPath=/dev/null",
    "-c",
    "core.fsmonitor=false",
];

/// `git -C <dir>`, its input closed, for a command the forms below don't
/// cover (an environment or flags of its own, its output read as it is).
pub fn git_command(dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).stdin(Stdio::null());
    cmd
}

/// Runs git in `dir`: what it printed, trimmed, or an error carrying the
/// first line of what it said went wrong; `.ok()` for none when it fails.
pub fn git(dir: &Path, args: &[&str]) -> Result<String> {
    git_text(git_command(dir).args(args))
}

/// As `git`, with `GIT_SAFE`'s flags.
pub fn git_safe(dir: &Path, args: &[&str]) -> Result<String> {
    git_text(git_command(dir).args(GIT_SAFE).args(args))
}

/// What a git command built from `git_command` printed, trimmed.
pub fn git_text(cmd: &mut Command) -> Result<String> {
    Ok(run_command(cmd)?.trim().to_string())
}

/// As `git`, its output as git printed it, for output whose spaces count (a
/// diff, `status --porcelain`).
pub fn git_as_is(dir: &Path, args: &[&str]) -> Result<String> {
    run_command(git_command(dir).args(args))
}

/// As `git`, its output's bytes as they are (a file's text may not be UTF-8).
pub fn git_bytes(dir: &Path, args: &[&str]) -> Result<Vec<u8>> {
    Ok(output_of(git_command(dir).args(args))?.stdout)
}

/// git in `dir`, what it said (its output and its errors) whichever way it
/// went, and whether it succeeded: for a push or a fetch, whose reasons
/// span lines.
pub fn git_said(dir: &Path, args: &[&str]) -> (bool, String) {
    match git_command(dir).args(args).output() {
        Ok(out) => {
            let mut said = String::from_utf8_lossy(&out.stdout).into_owned();
            said.push_str(&String::from_utf8_lossy(&out.stderr));
            (out.status.success(), said.trim().to_string())
        }
        Err(e) => (false, e.to_string()),
    }
}

/// As `git_safe`, for an async task (the server's looks), given up after
/// `timeout`.
#[cfg(feature = "async")]
pub async fn git_async(dir: &Path, args: &[&str], timeout: std::time::Duration) -> Result<String> {
    let mut cmd = tokio::process::Command::from(git_command(dir));
    cmd.args(GIT_SAFE).args(args).kill_on_drop(true);
    let output = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| anyhow!("git timed out"))??;
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr);
        let first = said.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        return Err(anyhow!("git failed: {}", first.trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Runs git in `dir`, its output thrown away; whether it succeeded.
pub fn git_ok(dir: &Path, args: &[&str]) -> bool {
    git_command(dir)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// git with a config and an identity of its own, none of yours, nor any of
/// a repository the caller runs in, for a scratch repository: a test's.
pub fn git_isolated(dir: &Path) -> Command {
    let mut cmd = git_command(dir);
    cmd.args([
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "commit.gpgSign=false",
        // Read from the home whatever GIT_CONFIG_GLOBAL says.
        "-c",
        "core.excludesFile=/dev/null",
        "-c",
        "core.attributesFile=/dev/null",
    ]);
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        cmd.env_remove(var);
    }
    cmd.envs([
        ("GIT_CONFIG_GLOBAL", "/dev/null"),
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_AUTHOR_NAME", "t"),
        ("GIT_AUTHOR_EMAIL", "t@example.com"),
        ("GIT_COMMITTER_NAME", "t"),
        ("GIT_COMMITTER_EMAIL", "t@example.com"),
    ]);
    cmd
}

/// For a test: `git_isolated` in `dir`, its output trimmed, panicking with
/// what git said when it fails.
pub fn git_scratch(dir: &Path, args: &[&str]) -> String {
    git_text(git_isolated(dir).args(args)).unwrap_or_else(|e| panic!("git {args:?}: {e}"))
}
