//! M-p's "Restart Switchboard on the latest build on disk": what runs before
//! the restart (src/mux/upgrade.rs) so that it has something new to restart
//! on. A binary built from a clone knows the main clone of the checkout it
//! was built in (build.rs's `SWITCHBOARD_MAIN_CLONE`, src/mux/main_clone.rs),
//! and builds that, not the checkout itself: a worktree is removed once its
//! work lands, and what counts then is the main clone. The main clone is
//! brought up to date first: on its default branch (`origin/HEAD`, else
//! `main`) with nothing uncommitted, or nothing is built and the restart
//! says why; then `git fetch origin <default>` and a fast-forward to it,
//! refused when the clone has commits origin lacks, and, when the fetch
//! fails (offline) or Settings' GitHub is off, which leaves it unmade, the
//! clone checked against what it last fetched and built as it is, with a
//! note saying so. It is
//! built with `cargo build --release --locked` (in the target directory the
//! binary was built in when that was the main clone's, else cargo's own),
//! as `swb build` and the README's install do, and the result put in place of the binary the
//! server runs when that isn't the build's own file (a copy installed in
//! `~/.local/bin`). What it does as it goes is told to the server as
//! `Report`s, cargo's messages read as they come, for the build row
//! (src/mux/build_row.rs). Cargo runs the checkout's own code, so it builds only
//! a checkout `src/trust.rs` finds yours or listed in `trusted-dirs`, as
//! `switchboard try` does, never someone else's PR. A release build has no checkout here, so it builds
//! nothing and restarts on the binary on disk as it is (`switchboard update`
//! is how a new release gets there).
//!
//! Cargo rebuilds only what changed, so with nothing new the build takes a
//! second and leaves its file alone. Whether there is anything new is then
//! whether the binary at the server's path is still the file the server
//! started from (`Stamp`: its device, inode, size and modified time, noted
//! as it starts): when it is, nothing restarts. One put there by hand since
//! (a build of another worktree installed) is restarted on as it is, with
//! nothing built over it.
//!
//! A server running from a cargo target directory (`target/debug`, or one
//! `CARGO_TARGET_DIR` shares between worktrees) never restarts on that
//! path: any worktree's build or `cargo test` writes over it, so a file
//! found there may be another branch's build, and one put there would be
//! written over again. The build goes instead to the installed build
//! (`~/.local/bin/switchboard`, where install.sh puts it) when there is
//! one, else to a copy of the server's own in `~/.cache/switchboard/build`,
//! and the restart runs from there, whether or not the build changed
//! anything, so the server leaves the target directory on the first press,
//! unless a build was installed there since the server started, which is
//! restarted on as it is.
//! The link `switchboard try` makes at the first into the worktree's target
//! directory is pointed at the second, which the next try links back. With
//! nothing to build it restarts on the installed build as it is, unless
//! that is a link into a target directory; with none, it says there is
//! nothing new while its own file is unchanged, and refuses once another
//! build has written over it.
//!
//! `SWITCHBOARD_CHECKOUT` in the server's environment names another
//! checkout to build as it is (with cargo's own target directory), or,
//! empty, none: `switchboard try` names its worktree, so a try's restart
//! builds the worktree under test, and the tests name none, so that nothing
//! they do builds this repository. `SWITCHBOARD_MAIN_CLONE` there names a
//! main clone to bring up to date and build, ahead of it, for the tests
//! (cargo gives a test both of build.rs's values, which the tests' servers
//! set empty).
//! A binary without a main clone (built from a source archive) builds the
//! checkout it was built in, as it is.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use crate::paths;

/// The setting that turns the build off, leaving the restart on what is on
/// disk.
pub const SETTING: &str = "build_before_restart";

/// Which file a path names, and as it was then.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stamp {
    dev: u64,
    ino: u64,
    len: u64,
    modified: Option<SystemTime>,
}

impl Stamp {
    pub fn of(path: &Path) -> Option<Stamp> {
        let meta = std::fs::metadata(path).ok()?;
        Some(Stamp {
            dev: meta.dev(),
            ino: meta.ino(),
            len: meta.len(),
            modified: meta.modified().ok(),
        })
    }
}

/// The server's binary as it started.
static STARTED: OnceLock<Option<Stamp>> = OnceLock::new();

/// The binary at the server's path as M-p's last build left it, so a
/// build of its own that the restart refused is built over again rather
/// than taken for one installed by hand.
static LAST_BUILT: Mutex<Option<Stamp>> = Mutex::new(None);

/// Notes the binary the server starts from, to tell later whether a newer
/// one has taken its place.
pub fn remember(binary: &Path) {
    let _ = STARTED.set(Stamp::of(binary));
    let _ = INSTALLED.set(Stamp::of(&installed_path(&paths::home())));
}

/// The installed build as the server started, to tell one put in place
/// since.
static INSTALLED: OnceLock<Option<Stamp>> = OnceLock::new();

/// Whether the file at `binary` is no longer the one the server started
/// from (or that isn't known).
pub fn is_new(binary: &Path) -> bool {
    match STARTED.get() {
        Some(Some(started)) => Stamp::of(binary).as_ref() != Some(started),
        _ => true,
    }
}

/// What M-p's restart builds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkout {
    pub dir: PathBuf,
    /// The target directory to build in, or cargo's own.
    pub target: Option<PathBuf>,
    /// A main clone, brought up to date before it is built.
    pub main_clone: bool,
}

fn nonempty(s: &str) -> Option<PathBuf> {
    (!s.trim().is_empty()).then(|| PathBuf::from(s.trim()))
}

/// The checkout M-p's restart builds, as the module's comment says;
/// `None` for a release build.
pub fn checkout() -> Option<Checkout> {
    let given = |var| std::env::var(var).ok();
    choose(
        given("SWITCHBOARD_MAIN_CLONE").as_deref(),
        given("SWITCHBOARD_CHECKOUT").as_deref(),
        env!("SWITCHBOARD_MAIN_CLONE"),
        env!("SWITCHBOARD_CHECKOUT"),
        env!("SWITCHBOARD_TARGET_DIR"),
    )
}

/// `checkout`'s choice, from the environment's main clone and checkout
/// and the ones the build recorded.
fn choose(
    env_main: Option<&str>,
    env_checkout: Option<&str>,
    main: &str,
    built_in: &str,
    target: &str,
) -> Option<Checkout> {
    if let Some(dir) = env_main.and_then(nonempty) {
        return Some(Checkout {
            dir,
            target: None,
            main_clone: true,
        });
    }
    if let Some(given) = env_checkout {
        return nonempty(given).map(|dir| Checkout {
            dir,
            target: None,
            main_clone: false,
        });
    }
    let built_in = nonempty(built_in)?;
    let target = nonempty(target);
    Some(match nonempty(main) {
        Some(main) => Checkout {
            target: target.filter(|_| main == built_in),
            dir: main,
            main_clone: true,
        },
        None => Checkout {
            dir: built_in,
            target,
            main_clone: false,
        },
    })
}

/// The `cargo` to build with: the PATH's, else the one the build had.
fn cargo() -> PathBuf {
    crate::util::which("cargo")
        .or_else(|| Some(PathBuf::from(env!("SWITCHBOARD_CARGO"))).filter(|p| p.is_file()))
        .unwrap_or_else(|| PathBuf::from("cargo"))
}

/// Whether `checkout` may be built (or brought up to date, which runs its
/// git hooks): it is there, and its code is yours.
fn may_build(checkout: &Path) -> Result<(), String> {
    if !checkout.join("Cargo.toml").is_file() {
        return Err(format!(
            "{}, the checkout it was built from, isn't there any more",
            paths::tilde(checkout)
        ));
    }
    // Cargo runs the checkout's build.rs and proc macros with your
    // credentials: only for code that is yours, as `switchboard try` asks.
    if crate::trust::is_untrusted(checkout) {
        return Err(format!(
            "{} holds a PR wtpr --untrusted set up, whose code runs only in switchboard try --sealed",
            paths::tilde(checkout)
        ));
    }
    crate::trust::yours_or_listed(&paths::home(), checkout)
}

/// Brings the main clone `dir` up to date with its origin, as the module's
/// comment says, or says why it can't be built; gives a note when the
/// fetch failed, or wasn't made (`fetch` false: Settings' GitHub off),
/// and it is built as it is.
fn update(dir: &Path, fetch: bool) -> Result<Option<String>, String> {
    let name = paths::tilde(dir);
    let git = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .stdin(Stdio::null())
            .output()
    };
    let text = |args: &[&str]| {
        git(args)
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let first_line = |o: &std::process::Output| {
        let err = String::from_utf8_lossy(&o.stderr);
        err.lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("git failed")
            .trim_start_matches("fatal: ")
            .to_string()
    };
    let default = text(&["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
        .and_then(|r| r.strip_prefix("origin/").map(String::from))
        .unwrap_or_else(|| "main".to_string());
    let branch = text(&["symbolic-ref", "--short", "HEAD"]);
    if branch.as_deref() != Some(default.as_str()) {
        return Err(format!(
            "{name} is on {}, not {default}; switch it back to restart on a new build",
            branch.unwrap_or_else(|| "a detached HEAD".to_string())
        ));
    }
    let status = text(&["status", "--porcelain", "--untracked-files=normal"])
        .ok_or_else(|| format!("git couldn't read {name}"))?;
    if !status.is_empty() {
        return Err(format!(
            "{name} has uncommitted changes; commit or stash them to restart on a new build"
        ));
    }
    // Fetched only while Settings' GitHub is on; off, or offline, the
    // clone is checked against what it last fetched and built as it is.
    let note = match fetch {
        false => Some("(origin not fetched: GitHub is turned off in Settings)".to_string()),
        true => {
            crate::requests::count("GitHub", "git", "restart");
            // Into origin/<default> by name, whatever origin's refspec fetches.
            let refspec = format!("+refs/heads/{default}:refs/remotes/origin/{default}");
            let fetch = git(&["fetch", "-q", "origin", &refspec])
                .map_err(|e| format!("git couldn't be run ({e})"))?;
            (!fetch.status.success())
                .then(|| format!("(couldn't fetch origin: {})", first_line(&fetch)))
        }
    };
    let upstream = format!("origin/{default}");
    if text(&["rev-parse", "--verify", "-q", &upstream]).is_none() {
        return Err(format!(
            "{name} has no {upstream} to check it against; fetch it to restart on a new build"
        ));
    }
    let behind =
        git(&["merge-base", "--is-ancestor", "HEAD", &upstream]).is_ok_and(|o| o.status.success());
    if !behind {
        return Err(format!(
            "{name} has commits {upstream} doesn't; push or reset them to restart on a new build"
        ));
    }
    // Not fetched: built as it is, not moved to what origin was once.
    if note.is_some() {
        return Ok(note);
    }
    let merge = git(&["merge", "-q", "--ff-only", &upstream])
        .map_err(|e| format!("git couldn't be run ({e})"))?;
    if !merge.status.success() {
        return Err(format!(
            "{name} couldn't be brought up to {upstream}: {}",
            first_line(&merge)
        ));
    }
    Ok(note)
}

/// Builds `checkout` as the module's comment says and gives the binary it
/// made, or why it couldn't, in words. Cargo's messages are read as they
/// come, each crate it has built (fresh ones too) told to `report` at most
/// twice a second (`Counter`), and how many there were kept for the next
/// build's meter (`restart-build.json`).
pub fn build(
    checkout: &Path,
    target: Option<&Path>,
    report: &dyn Fn(Report),
) -> Result<PathBuf, String> {
    may_build(checkout)?;
    let cargo = cargo();
    let mut cmd = Command::new(&cargo);
    // rustup's cargo runs the toolchain's, which finds rustc on the PATH: a
    // server started without rustup's folder on its PATH (one taken from the
    // build) has to be given it, or cargo can't run `rustc -vV`.
    if let Some(dir) = cargo.parent().filter(|d| !d.as_os_str().is_empty()) {
        let rest = std::env::var_os("PATH").unwrap_or_default();
        let path = std::env::join_paths(
            std::iter::once(dir.to_path_buf()).chain(std::env::split_paths(&rest)),
        )
        .unwrap_or(rest);
        cmd.env("PATH", path);
    }
    cmd.args([
        "build",
        "--release",
        "--locked",
        "--message-format=json-render-diagnostics",
    ])
    .current_dir(checkout)
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    if let Some(target) = target {
        cmd.env("CARGO_TARGET_DIR", target);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("cargo couldn't be run ({e})"))?;
    // Read beside stdout, so cargo never waits on a full pipe.
    let stderr = child.stderr.take().map(|mut err| {
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = std::io::Read::read_to_string(&mut err, &mut text);
            text
        })
    });
    let mut counter = Counter::default();
    if let Some(out) = child.stdout.take() {
        for line in std::io::BufRead::lines(std::io::BufReader::new(out)) {
            let Ok(line) = line else { break };
            if let Some(n) = counter.line(&line, Instant::now()) {
                report(Report::Built(n));
            }
        }
    }
    let status = child
        .wait()
        .map_err(|e| format!("cargo couldn't be run ({e})"))?;
    let stderr = stderr.and_then(|t| t.join().ok()).unwrap_or_default();
    if !status.success() {
        report(Report::Failed(error_lines(&stderr)));
        return Err(failure(&stderr));
    }
    report(Report::Built(counter.count));
    keep_count(checkout, counter.count);
    counter
        .binary
        .ok_or_else(|| "cargo didn't say where it put the binary".to_string())
}

/// What M-p's build tells the server as it goes (src/mux/build_row.rs
/// shows it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Report {
    /// The main clone brought up to date: how many commits that brought,
    /// `None` when origin wasn't fetched, and how long it took.
    Fetched {
        commits: Option<usize>,
        took: Duration,
    },
    /// The build begun in `dir`, at `head` ("main at 1a2b3c4"), with how
    /// many crates the last build of it counted, `None` the first time.
    Building {
        dir: PathBuf,
        head: String,
        of: Option<usize>,
    },
    /// The crates built so far.
    Built(usize),
    /// The build going in place of the binary at this path.
    Installing(PathBuf),
    /// Cargo failed: its error's lines.
    Failed(Vec<String>),
}

/// Counts cargo's `compiler-artifact` messages as they come and notes the
/// binary's, saying how many are done at most twice a second.
#[derive(Debug, Default)]
struct Counter {
    count: usize,
    binary: Option<PathBuf>,
    said: Option<Instant>,
}

/// How often the count is told while cargo builds.
const REPORT_EVERY: Duration = Duration::from_millis(500);

impl Counter {
    /// Takes one of cargo's lines, and gives the count when it is due.
    fn line(&mut self, line: &str, now: Instant) -> Option<usize> {
        let Ok(m) = serde_json::from_str::<serde_json::Value>(line) else {
            return None;
        };
        if m["reason"] != "compiler-artifact" {
            return None;
        }
        self.count += 1;
        if m["target"]["name"] == "switchboard"
            && let Some(exe) = m["executable"].as_str()
        {
            self.binary = Some(PathBuf::from(exe));
        }
        if self
            .said
            .is_some_and(|at| now.duration_since(at) < REPORT_EVERY)
        {
            return None;
        }
        self.said = Some(now);
        Some(self.count)
    }
}

/// The file that keeps how many crates each checkout's last build counted.
fn counts_file() -> PathBuf {
    paths::state_dir().join("restart-build.json")
}

/// How many crates `checkout`'s last build counted.
pub fn last_count(checkout: &Path) -> Option<usize> {
    let text = crate::util::read_text(counts_file())?;
    let all: serde_json::Value = serde_json::from_str(&text).ok()?;
    all.get(checkout.to_string_lossy().as_ref())?
        .as_u64()
        .map(|n| n as usize)
}

/// Keeps how many crates `checkout`'s build counted, for the next one's
/// meter.
fn keep_count(checkout: &Path, count: usize) {
    let file = counts_file();
    let mut all = crate::util::read_text(&file)
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    all[checkout.to_string_lossy().as_ref()] = serde_json::json!(count);
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = crate::util::write_atomic(&file, all.to_string()) {
        eprintln!(
            "switchboard server: {} couldn't be written ({e})",
            paths::tilde(&file)
        );
    }
}

/// What a failed build says, in a line: cargo's first error, else its last
/// line.
fn failure(stderr: &str) -> String {
    let lines = stderr.lines().map(str::trim).filter(|l| !l.is_empty());
    lines
        .clone()
        .find(|l| l.starts_with("error"))
        .or_else(|| lines.clone().next_back())
        .map(|l| l.trim_start_matches("error: ").to_string())
        .unwrap_or_else(|| "cargo failed".to_string())
}

/// A failed build's error, as cargo printed it: from its first `error`
/// line, at most `ERROR_LINES` lines, for the build's steps to show.
fn error_lines(stderr: &str) -> Vec<String> {
    let lines: Vec<&str> = stderr.lines().collect();
    let from = lines
        .iter()
        .position(|l| l.trim_start().starts_with("error"))
        .unwrap_or_else(|| lines.len().saturating_sub(ERROR_LINES));
    lines[from..]
        .iter()
        .map(|l| l.trim_end().to_string())
        .filter(|l| !l.is_empty())
        .take(ERROR_LINES)
        .collect()
}

const ERROR_LINES: usize = 8;

/// Puts `built` in place of the server's `binary` when the server doesn't
/// run the build's own file (or a link to it) and the two differ.
pub fn install(built: &Path, binary: &Path) -> Result<(), String> {
    let target = std::fs::canonicalize(binary).unwrap_or_else(|_| binary.to_path_buf());
    let built = std::fs::canonicalize(built).unwrap_or_else(|_| built.to_path_buf());
    if target == built {
        return Ok(());
    }
    let bytes = std::fs::read(&built)
        .map_err(|e| format!("{} couldn't be read ({e})", paths::tilde(&built)))?;
    if std::fs::read(&target).is_ok_and(|now| now == bytes) {
        return Ok(());
    }
    crate::update::install::replace(&target, &bytes)
        .map(|_| ())
        .map_err(|e| format!("{e:#}"))
}

/// Whether `binary` is in a cargo target directory: its real path has an
/// ancestor holding cargo's `CACHEDIR.TAG` (as build.rs finds one), or lies
/// under the target directory this build was made in.
pub fn in_target_dir(binary: &Path) -> bool {
    let real = std::fs::canonicalize(binary).unwrap_or_else(|_| binary.to_path_buf());
    let tagged = |dir: &Path| {
        std::fs::read_to_string(dir.join("CACHEDIR.TAG")).is_ok_and(|tag| tag.contains("cargo"))
    };
    let built_in = nonempty(env!("SWITCHBOARD_TARGET_DIR"));
    real.ancestors().skip(1).any(tagged)
        || built_in.is_some_and(|target| {
            let target = std::fs::canonicalize(&target).unwrap_or(target);
            real.starts_with(target)
        })
}

/// install.sh's place for the installed build, and where `switchboard
/// try` links its home's to the worktree's build.
fn installed_path(home: &Path) -> PathBuf {
    home.join(".local/bin/switchboard")
}

/// The installed build, when there is one to restart on as it is: not a
/// link into a cargo target directory (as a try's is).
fn installed(home: &Path) -> Option<PathBuf> {
    let path = installed_path(home);
    (path.exists() && !in_target_dir(&path)).then_some(path)
}

/// The path M-p's restart execs: the server's own `binary`, unless that is
/// in a cargo target directory, which other worktrees' builds and tests
/// write over; then the installed build's path when there is one (a link
/// into a target directory included, which the build replaces), else a
/// copy of the server's own in the cache.
pub fn restart_on(home: &Path, binary: &Path) -> PathBuf {
    if !in_target_dir(binary) {
        return binary.to_path_buf();
    }
    let path = installed_path(home);
    if std::fs::symlink_metadata(&path).is_ok() {
        return path;
    }
    cache_copy(home)
}

/// The copy of the server's own in the cache, for a restart off a target
/// directory with no installed build.
fn cache_copy(home: &Path) -> PathBuf {
    home.join(".cache/switchboard/build/switchboard")
}

/// Puts `built` at `on`, the restart's path off a target directory. A link
/// there into a target directory (a try's) is pointed at the cache's copy,
/// the build put there, so the commands on the PATH and the server run the
/// same file outside it, and the next `switchboard try` can link it again.
fn put(home: &Path, built: &Path, on: &Path) -> Result<(), String> {
    let is_link = std::fs::symlink_metadata(on).is_ok_and(|m| m.file_type().is_symlink());
    let relink = is_link && in_target_dir(on);
    let to = if relink {
        cache_copy(home)
    } else {
        on.to_path_buf()
    };
    if let Some(folder) = to.parent() {
        std::fs::create_dir_all(folder)
            .map_err(|e| format!("{} couldn't be made ({e})", paths::tilde(folder)))?;
    }
    install(built, &to)?;
    if !relink {
        return Ok(());
    }
    let folder = on.parent().unwrap_or(Path::new("."));
    let new = folder.join(format!(".switchboard.link-{}", std::process::id()));
    let _ = std::fs::remove_file(&new);
    std::os::unix::fs::symlink(&to, &new)
        .and_then(|()| std::fs::rename(&new, on))
        .map_err(|e| {
            let _ = std::fs::remove_file(&new);
            format!(
                "{} couldn't be pointed at the build ({e})",
                paths::tilde(on)
            )
        })
}

/// What M-p's build gives: the binary to restart on, `None` when there is
/// nothing new, and a note for the message after the restart (the fetch
/// that failed), else empty.
#[derive(Debug, Default)]
pub struct Brought {
    pub on: Option<PathBuf>,
    pub note: String,
}

/// Brings the main clone up to date when `checkout` is one, telling
/// `report` what that brought and once it is about to build, then builds
/// it.
fn make(
    checkout: &Checkout,
    report: &dyn Fn(Report),
    note: &mut String,
) -> Result<PathBuf, String> {
    let dir = &checkout.dir;
    let git = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    if checkout.main_clone {
        may_build(dir)?;
        let fetch = crate::integrations::ensure_on("github", "GitHub").is_ok();
        let started = Instant::now();
        let before = git(&["rev-parse", "HEAD"]);
        let not_fetched = update(dir, fetch)?;
        let commits = before
            .filter(|_| not_fetched.is_none())
            .and_then(|b| git(&["rev-list", "--count", &format!("{b}..HEAD")]))
            .and_then(|n| n.parse().ok());
        *note = not_fetched.unwrap_or_default();
        report(Report::Fetched {
            commits,
            took: started.elapsed(),
        });
    }
    let head = match (
        git(&["symbolic-ref", "--short", "HEAD"]),
        git(&["rev-parse", "--short", "HEAD"]),
    ) {
        (Some(branch), Some(hash)) => format!("{branch} at {hash}"),
        (None, Some(hash)) => format!("at {hash}"),
        _ => String::new(),
    };
    report(Report::Building {
        dir: dir.clone(),
        head,
        of: last_count(dir),
    });
    build(dir, checkout.target.as_deref(), report)
}

/// Builds and installs as the module's comment says (unless `build` is
/// off), and gives the binary to restart on, or `None` when there is
/// nothing new. A binary already put at the server's path since it started
/// (a build of another checkout installed by hand) is the latest one, so
/// nothing is built over it, unless that path is in a cargo target
/// directory, where the restart moves to `restart_on` as the module's
/// comment says.
pub fn bring_up_to_date(
    home: &Path,
    binary: &Path,
    build_first: bool,
    report: &dyn Fn(Report),
) -> Result<Brought, String> {
    let to_build = checkout().filter(|_| build_first);
    let mut note = String::new();
    let brought = |on: Option<PathBuf>, note: String| Ok(Brought { on, note });
    if in_target_dir(binary) {
        let Some(to_build) = to_build else {
            return match installed(home) {
                Some(installed) => brought(Some(installed), note),
                None if !is_new(binary) => brought(None, note),
                None => Err(
                    "Switchboard runs from a cargo target directory, which other worktrees' builds write over, and has no checkout to build; install a build (install.sh) to restart on it"
                        .to_string(),
                ),
            };
        };
        let mut last_built = LAST_BUILT.lock().unwrap_or_else(|e| e.into_inner());
        // A build installed since the server started (another worktree's,
        // as this repository's CLAUDE.md installs one before M-p) is the
        // latest: restarted on as it is, with nothing built over it.
        if let Some(installed) = installed(home) {
            let now = Stamp::of(&installed);
            let since_start = INSTALLED.get().is_some_and(|started| *started != now);
            if since_start && *last_built != now {
                return brought(Some(installed), note);
            }
        }
        let on = restart_on(home, binary);
        let built = make(&to_build, report, &mut note)?;
        report(Report::Installing(on.clone()));
        put(home, &built, &on)?;
        *last_built = Stamp::of(&on);
        return brought(Some(on), note);
    }
    let mut last_built = LAST_BUILT.lock().unwrap_or_else(|e| e.into_inner());
    let now = Stamp::of(binary);
    let installed_by_hand = now.is_some() && is_new(binary) && *last_built != now;
    if installed_by_hand {
        return brought(Some(binary.to_path_buf()), note);
    }
    if let Some(to_build) = to_build {
        let built = make(&to_build, report, &mut note)?;
        report(Report::Installing(binary.to_path_buf()));
        install(&built, binary)?;
        *last_built = Stamp::of(binary);
    }
    brought(is_new(binary).then(|| binary.to_path_buf()), note)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_cargos_artifacts_as_they_come_and_says_so_twice_a_second() {
        let messages = [
            r#"{"reason":"compiler-artifact","target":{"name":"serde","kind":["lib"]},"executable":null}"#,
            "not json",
            r#"{"reason":"build-script-executed","package_id":"x"}"#,
            r#"{"reason":"compiler-artifact","target":{"name":"libc","kind":["lib"]},"executable":null}"#,
            r#"{"reason":"compiler-artifact","target":{"name":"switchboard","kind":["lib"]},"executable":null}"#,
            r#"{"reason":"compiler-artifact","target":{"name":"switchboard","kind":["bin"]},"executable":"/t/release/switchboard"}"#,
            r#"{"reason":"build-finished","success":true}"#,
        ];
        let start = Instant::now();
        let ms = |n: u64| start + Duration::from_millis(n);
        let at = [0, 10, 20, 100, 600, 700, 800];
        let mut counter = Counter::default();
        let said: Vec<Option<usize>> = messages
            .iter()
            .zip(at)
            .map(|(m, t)| counter.line(m, ms(t)))
            .collect();
        assert_eq!(
            said,
            [Some(1), None, None, None, Some(3), None, None],
            "the first at once, then none within half a second"
        );
        assert_eq!(counter.count, 4);
        assert_eq!(
            counter.binary,
            Some(PathBuf::from("/t/release/switchboard"))
        );
    }

    #[test]
    fn a_failures_lines_start_at_cargos_first_error() {
        let stderr = "   Compiling switchboard v0.2.0\nerror[E0308]: mismatched types\n  --> src/x.rs:1:1\n\nerror: could not compile\n";
        assert_eq!(
            error_lines(stderr),
            [
                "error[E0308]: mismatched types",
                "  --> src/x.rs:1:1",
                "error: could not compile"
            ]
        );
        assert_eq!(error_lines("odd\n"), ["odd"]);
    }

    #[test]
    fn a_failure_is_cargos_first_error() {
        let stderr = "   Compiling switchboard v0.2.0\nerror: could not compile `switchboard` (bin \"switchboard\") due to 1 previous error\nerror: another\n";
        assert_eq!(
            failure(stderr),
            "could not compile `switchboard` (bin \"switchboard\") due to 1 previous error"
        );
        assert_eq!(failure("something odd\n"), "something odd");
        assert_eq!(failure(""), "cargo failed");
    }

    #[test]
    fn a_binary_is_new_once_another_file_takes_its_place() {
        let dir = std::env::temp_dir().join(format!("sb-latest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("switchboard");
        std::fs::write(&file, "one").unwrap();
        let before = Stamp::of(&file);
        assert_eq!(Stamp::of(&file), before);
        std::fs::remove_file(&file).unwrap();
        std::fs::write(&file, "two!").unwrap();
        assert_ne!(Stamp::of(&file), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A temporary folder of the test's own, emptied first.
    fn fresh(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sb-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const CARGO_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n# This file is a cache directory tag created by cargo.\n";

    #[test]
    fn a_binary_under_a_cachedir_tag_is_in_a_target_directory() {
        let dir = fresh("target-dir");
        std::fs::create_dir_all(dir.join("target/debug")).unwrap();
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("target/CACHEDIR.TAG"), CARGO_TAG).unwrap();
        std::fs::write(dir.join("target/debug/switchboard"), "").unwrap();
        std::fs::write(dir.join("bin/switchboard"), "").unwrap();
        assert!(in_target_dir(&dir.join("target/debug/switchboard")));
        assert!(!in_target_dir(&dir.join("bin/switchboard")));
        // Another program's cache tag isn't cargo's.
        std::fs::write(
            dir.join("CACHEDIR.TAG"),
            "Signature: 8a477f597d28d172789f06886806bc55\n",
        )
        .unwrap();
        assert!(!in_target_dir(&dir.join("bin/switchboard")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_restart_from_a_target_directory_lands_on_the_installed_build_else_a_copy() {
        let dir = fresh("restart-on");
        let home = dir.join("home");
        std::fs::create_dir_all(dir.join("target/debug")).unwrap();
        std::fs::write(dir.join("target/CACHEDIR.TAG"), CARGO_TAG).unwrap();
        let binary = dir.join("target/debug/switchboard");
        std::fs::write(&binary, "").unwrap();
        assert_eq!(
            restart_on(&home, &binary),
            home.join(".cache/switchboard/build/switchboard")
        );
        // The link `switchboard try` makes into a target directory is
        // pointed at the cache's copy, the build put there, not written
        // through.
        let link = home.join(".local/bin/switchboard");
        std::fs::create_dir_all(home.join(".local/bin")).unwrap();
        std::os::unix::fs::symlink(&binary, &link).unwrap();
        assert_eq!(restart_on(&home, &binary), link);
        let built = dir.join("built");
        let new = "#!/bin/sh\necho 'switchboard 0.0.1 (x)'\n";
        std::fs::write(&built, new).unwrap();
        put(&home, &built, &link).unwrap();
        let copy = home.join(".cache/switchboard/build/switchboard");
        assert_eq!(std::fs::read_link(&link).unwrap(), copy);
        assert_eq!(std::fs::read_to_string(&copy).unwrap(), new);
        assert_eq!(std::fs::read_to_string(&binary).unwrap(), "");
        std::fs::remove_file(&link).unwrap();
        std::fs::write(home.join(".local/bin/switchboard"), "").unwrap();
        assert_eq!(
            restart_on(&home, &binary),
            home.join(".local/bin/switchboard")
        );
        let elsewhere = dir.join("switchboard");
        std::fs::write(&elsewhere, "").unwrap();
        assert_eq!(restart_on(&home, &elsewhere), elsewhere);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_restart_builds_the_main_clone_unless_the_environment_names_a_checkout() {
        let main = |dir: &str, target: Option<&str>| Checkout {
            dir: PathBuf::from(dir),
            target: target.map(PathBuf::from),
            main_clone: true,
        };
        // Built in a worktree: its main clone, in cargo's own target.
        assert_eq!(
            choose(None, None, "/c/main", "/c/main/wt", "/c/main/wt/target"),
            Some(main("/c/main", None))
        );
        // Built in the main clone: there, in the target it used.
        assert_eq!(
            choose(None, None, "/c/main", "/c/main", "/t"),
            Some(main("/c/main", Some("/t")))
        );
        // No main clone known: the checkout as it is.
        assert_eq!(
            choose(None, None, "", "/c/src", "/t"),
            Some(Checkout {
                dir: PathBuf::from("/c/src"),
                target: Some(PathBuf::from("/t")),
                main_clone: false
            })
        );
        // A try names its worktree; the tests name none.
        assert_eq!(
            choose(None, Some("/c/main/wt"), "/c/main", "/c/main", "/t"),
            Some(Checkout {
                dir: PathBuf::from("/c/main/wt"),
                target: None,
                main_clone: false
            })
        );
        assert_eq!(choose(None, Some(""), "/c/main", "/c/main", "/t"), None);
        assert_eq!(
            choose(Some("/m"), Some(""), "/c/main", "/c/main", "/t"),
            Some(main("/m", None))
        );
        // A release build has nothing to build.
        assert_eq!(choose(None, None, "", "", ""), None);
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-C"])
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn the_main_clone_is_brought_up_to_date_only_on_its_default_branch_and_clean() {
        let dir = fresh("update");
        let origin = dir.join("origin.git");
        let main = dir.join("main");
        let other = dir.join("other");
        git(
            &dir,
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                origin.to_str().unwrap(),
            ],
        );
        git(
            &dir,
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                other.to_str().unwrap(),
            ],
        );
        git(&other, &["commit", "-q", "--allow-empty", "-m", "one"]);
        git(&other, &["push", "-q", "origin", "HEAD:main"]);
        git(
            &dir,
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                main.to_str().unwrap(),
            ],
        );

        // Behind origin: fast-forwarded to it.
        git(&other, &["commit", "-q", "--allow-empty", "-m", "two"]);
        git(&other, &["push", "-q", "origin", "HEAD:main"]);
        assert_eq!(update(&main, true), Ok(None));
        assert_eq!(git(&main, &["log", "-1", "--format=%s"]), "two");

        // Another branch, or anything uncommitted: nothing done.
        git(&main, &["checkout", "-q", "-b", "item"]);
        let on_item = update(&main, true).unwrap_err();
        assert!(
            on_item.ends_with("is on item, not main; switch it back to restart on a new build"),
            "{on_item}"
        );
        git(&main, &["checkout", "-q", "main"]);
        std::fs::write(main.join("file"), "").unwrap();
        let dirty = update(&main, true).unwrap_err();
        assert!(dirty.contains("has uncommitted changes"), "{dirty}");
        std::fs::remove_file(main.join("file")).unwrap();

        // A commit origin lacks: refused.
        git(&main, &["commit", "-q", "--allow-empty", "-m", "mine"]);
        let ahead = update(&main, true).unwrap_err();
        assert!(ahead.contains("has commits origin/main doesn't"), "{ahead}");
        git(&main, &["reset", "-q", "--hard", "origin/main"]);

        // An origin that can't be reached: built as it is, with a note,
        // unless the clone has commits it last saw origin lack.
        std::fs::rename(&origin, dir.join("gone.git")).unwrap();
        let note = update(&main, true).unwrap().unwrap();
        assert!(note.starts_with("(couldn't fetch origin: "), "{note}");
        // Nor fetched with GitHub off, and not moved to what origin was.
        git(&main, &["reset", "-q", "--hard", "HEAD~1"]);
        let off = update(&main, false).unwrap().unwrap();
        assert!(off.contains("GitHub is turned off"), "{off}");
        assert_eq!(git(&main, &["log", "-1", "--format=%s"]), "one");
        git(&main, &["reset", "-q", "--hard", "origin/main"]);
        git(&main, &["commit", "-q", "--allow-empty", "-m", "mine"]);
        let ahead = update(&main, true).unwrap_err();
        assert!(ahead.contains("has commits origin/main doesn't"), "{ahead}");
        // No origin branch to check against: refused.
        git(&main, &["update-ref", "-d", "refs/remotes/origin/main"]);
        let unknown = update(&main, true).unwrap_err();
        assert!(unknown.contains("has no origin/main"), "{unknown}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn installs_a_build_only_where_it_differs() {
        let dir = std::env::temp_dir().join(format!("sb-install-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let built = dir.join("built");
        let link = dir.join("link");
        std::fs::write(&built, "#!/bin/sh\necho 'switchboard 0.0.1 (x)'\n").unwrap();
        std::os::unix::fs::symlink(&built, &link).unwrap();
        let before = Stamp::of(&built);
        // The server runs the build's own file, through a link: nothing to do.
        install(&built, &link).unwrap();
        assert_eq!(Stamp::of(&built), before);
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
