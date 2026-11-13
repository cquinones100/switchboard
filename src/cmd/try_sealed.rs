//! `switchboard try --sealed`: someone else's code, a fork's PR `wtpr
//! --untrusted` checked out, tried without any of it running outside a
//! sandbox. Ported from bin/switchboard-try's `--sealed`, which ran the
//! PR's setup, tmux server, everything tmux started and every tmux client
//! under macOS's sandbox; here every process of the try is in it: the build
//! (the PR's `build.rs` and proc macros run as cargo builds), the build's
//! own setup, its server and everything the server starts, and every client
//! of it, the attached terminal's and `--detached`'s prefix, since a
//! client is the PR's binary too. src/platform/sandbox.rs has the rules:
//! nothing in it reads your home or your per-user folders, writes beyond
//! the try's home and `/tmp`, reaches the network, connects to any socket
//! but its own, signals a process outside it, or asks the Keychain, the
//! clipboard or Launch Services. Linux has no sandbox without installing
//! another program, so there `--sealed` refuses, saying so.
//!
//! The home, `/tmp/sb-<hash>` as an open try's, holds nothing of yours: no
//! link into your home, a `~/.config/switchboard/settings.json` turning off
//! every feature that reaches a service or Claude (`SEALED_OFF`), an empty
//! `~/.claude` (Claude Code isn't signed in there), `~/Notes` with a few
//! made-up notes and `~/code` with two made-up repositories, one with a
//! branch for `M-d`, each with an origin of its own in the home. Its
//! environment is made from nothing, so no token or `SSH_AUTH_SOCK` comes
//! along. Its code can change anything in its home, so nothing outside the
//! sandbox writes there once any of it has run: a try still running is only
//! attached to (or `--run` in), and any other is stopped, removed (a link in
//! it removed as a link, never followed) and made again from nothing, laid
//! out in `/tmp/sb-<hash>.new`, which no sandbox may touch, and renamed into
//! place whole. A link the build is reached by
//! (`~/.local/bin/switchboard`) is made then, before the build, since none
//! may be made after it.
//!
//! Building runs the PR's code, so cargo builds inside the sandbox, offline
//! and locked, its target in the home: the toolchain is your main clone's
//! (as `rustup which` finds it there, run directly, never through rustup's
//! proxies, so the PR's `rust-toolchain.toml` names nothing that runs), and
//! the C compiler, git and the SDK are found outside (`platform::
//! sealed_build`), so nothing inside asks a shim that writes outside it.
//! The crates are fetched outside the sandbox, which `cargo fetch` can do
//! without running anything of theirs, only from crates.io, each with its
//! checksum in `Cargo.lock`: a git, path or other registry's dependency is
//! refused (`manifest_problem`, read with cargo's own TOML parser, and
//! `lock_problem`, which takes `Cargo.lock` only as cargo writes it),
//! since cargo would fetch it from wherever the PR says. They are fetched
//! into `~/.cache/switchboard/sealed/cargo`, which no sandbox can write,
//! with no environment or config of yours and from a folder with nothing in
//! it (cargo reads `.cargo/config.toml` from the folder it runs in and
//! those above), then copied into the home. A lockfile other than your main
//! clone's is fetched only with `--fetch`, once you have read its change,
//! as bin/switchboard-try installed lib's packages only with `--npm-ci`.
//!
//! `--run <command>...` runs a command there from the worktree, the PR's
//! tests (`--run cargo test`), with localhost and the tests' own sockets
//! allowed, and exits with its status. The sandbox's rules are written by
//! this program into `~/.cache/switchboard/sealed/` (which also marks the
//! home as sealed, where nothing in the try can write) and given to
//! `sandbox-exec` on each run; `try --clean` stops the server through
//! Switchboard's own binary in a sandbox that can reach that socket alone
//! (`call_main`), since whatever answers there, or a link put in its place,
//! is the try's to choose.

use std::fs;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, symlink};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Map, Value, json};

use super::try_::{self, Mode};
use crate::platform::{self, sandbox};
use crate::worktree::shell_quote;
use crate::{paths, requests, trust, util};

/// The settings a sealed try starts with: every feature that reaches a
/// service or Claude off. A PR's code ignoring them still meets the
/// sandbox's network rule, but the try then shows what works without them
/// rather than a screen of errors.
pub const SEALED_OFF: &[(&str, &str)] = &[
    ("github", "false"),
    ("linear", "false"),
    ("notifications", "false"),
    ("link_view_docs", "false"),
    ("chrome_profiles", "false"),
    ("standup_note", "false"),
    ("standup_summary", "false"),
    ("search_everything", "false"),
    ("search_slack", "false"),
    ("diff_order", "false"),
    ("diff_story", "false"),
    ("diff_story_ahead", "false"),
    ("diff_ask", "false"),
    ("diff_story_notes", "false"),
    ("slack_background", "false"),
    ("view_slack", "false"),
    ("view_calendar", "false"),
    ("calendar_rsvp", "false"),
    ("calendar_changes", "false"),
    ("next_meeting", "false"),
    ("tab_lookups", "false"),
    ("haiku_summaries", "false"),
    ("update_check", "\"off\""),
    ("auto_restore", "false"),
];

/// The crates.io index, as `Cargo.lock` names it.
const CRATES_IO: &[&str] = &[
    "registry+https://github.com/rust-lang/crates.io-index",
    "sparse+https://index.crates.io/",
];

/// Where the sealed tries' rules and the crates they fetch are kept: in
/// your cache, which nothing in a try can write.
fn sealed_dir() -> PathBuf {
    paths::cache_dir().join("sealed")
}

fn name_of(home: &Path) -> String {
    home.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The rules a try's every process runs under; their file also marks its
/// home as sealed.
fn profile_file(home: &Path) -> PathBuf {
    sealed_dir().join(format!("{}.sb", name_of(home)))
}

/// The rules `--run` runs under: localhost and the tests' sockets too.
fn run_profile_file(home: &Path) -> PathBuf {
    sealed_dir().join(format!("{}-run.sb", name_of(home)))
}

/// The rules Switchboard's own binary runs under to reach the try's server.
fn client_profile_file(home: &Path) -> PathBuf {
    sealed_dir().join(format!("{}-client.sb", name_of(home)))
}

/// What the crates in a `--run`'s home were fetched for: its worktree's
/// `Cargo.lock`, by its digest, so a later `--run` reuses the home only
/// while the lock is the same.
fn crates_file(home: &Path) -> PathBuf {
    sealed_dir().join(format!("{}.crates", name_of(home)))
}

fn lock_digest(wt: &Path) -> Option<String> {
    use sha1::{Digest, Sha1};
    let lock = fs::read(wt.join("Cargo.lock")).ok()?;
    Some(
        Sha1::digest(lock)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    )
}

/// Whether `home` is where a sealed try's home goes: the rules in your
/// cache are named by the home's name alone, so they belong to a home in
/// the tries' root and nowhere else, and a folder of the same name
/// elsewhere (a test's) never reaches them.
fn in_root(home: &Path) -> bool {
    home.parent() == Some(Path::new(try_::ROOT))
}

/// Whether the try in `home` was made with `--sealed`, by its rules' file
/// in your cache rather than anything in the home, which its code can
/// change.
pub fn is_sealed(home: &Path) -> bool {
    in_root(home) && profile_file(home).exists()
}

/// The homes your sealed tries have under `root`, by their rules' files,
/// whether or not they are still there.
pub fn homes(root: &Path) -> Vec<PathBuf> {
    if root != Path::new(try_::ROOT) {
        return Vec::new();
    }
    let mut found: Vec<PathBuf> = fs::read_dir(sealed_dir())
        .map(|d| {
            d.flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let home = name.strip_suffix(".sb")?;
                    let hex = home.strip_prefix("sb-")?;
                    (hex.len() == 8 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
                        .then(|| root.join(home))
                })
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found
}

/// The home being made for `home`: beside it, which no sandbox may touch.
fn staging(home: &Path) -> PathBuf {
    let mut name = home.as_os_str().to_owned();
    name.push(".new");
    PathBuf::from(name)
}

/// Removes a path, a folder whole, a link as a link.
fn remove(path: &Path) {
    match path.symlink_metadata() {
        Ok(m) if m.is_dir() => {
            let _ = fs::remove_dir_all(path);
        }
        Ok(_) => {
            let _ = fs::remove_file(path);
        }
        Err(_) => {}
    }
}

// ── Whether its crates may be fetched ───────────────────────────────────────

/// The package `Cargo.toml` names, or why its crates aren't fetched: read
/// with cargo's own TOML parser, every dependency must come from crates.io
/// (no `git`, `path` or `registry` of its own), and nothing may stand in for
/// a crate (`[patch]`, `[replace]`) or name a workspace elsewhere, since
/// cargo would fetch any of them from wherever the PR says, outside the
/// sandbox.
pub fn manifest_problem(text: &str) -> Result<String, String> {
    let manifest: toml::Table = text
        .parse()
        .map_err(|e| format!("Cargo.toml doesn't read as TOML: {e}"))?;
    for key in ["patch", "replace"] {
        if manifest.contains_key(key) {
            return Err(format!(
                "Cargo.toml has a [{key}] section, whose crates come from elsewhere"
            ));
        }
    }
    let package = manifest.get("package").and_then(|p| p.as_table());
    let Some(name) = package.and_then(|p| p.get("name")).and_then(|n| n.as_str()) else {
        return Err("Cargo.toml names no package".into());
    };
    if package.is_some_and(|p| p.contains_key("workspace")) {
        return Err("Cargo.toml names a workspace elsewhere".into());
    }
    /// A table's dependency tables, each named as Cargo.toml names it.
    fn deps_of<'a>(
        at: &str,
        table: &'a toml::Table,
        out: &mut Vec<(String, &'a toml::Table)>,
    ) -> Result<(), String> {
        for kind in [
            "dependencies",
            "dev-dependencies",
            "dev_dependencies",
            "build-dependencies",
            "build_dependencies",
        ] {
            if let Some(deps) = table.get(kind) {
                let deps = deps
                    .as_table()
                    .ok_or_else(|| format!("Cargo.toml's {at}{kind} isn't a table"))?;
                out.push((format!("{at}{kind}"), deps));
            }
        }
        Ok(())
    }
    let mut tables: Vec<(String, &toml::Table)> = Vec::new();
    deps_of("", &manifest, &mut tables)?;
    if let Some(targets) = manifest.get("target").and_then(|t| t.as_table()) {
        for (cfg, target) in targets {
            if let Some(target) = target.as_table() {
                deps_of(&format!("target.{cfg}."), target, &mut tables)?;
            }
        }
    }
    if let Some(workspace) = manifest.get("workspace").and_then(|w| w.as_table()) {
        if workspace
            .get("members")
            .and_then(|m| m.as_array())
            .is_some_and(|m| !m.is_empty())
        {
            return Err("Cargo.toml is a workspace of several packages".into());
        }
        if let Some(deps) = workspace.get("dependencies").and_then(|d| d.as_table()) {
            tables.push(("workspace.dependencies".into(), deps));
        }
    }
    for (at, deps) in tables {
        for (dep, spec) in deps {
            let Some(spec) = spec.as_table() else {
                if spec.is_str() {
                    continue;
                }
                return Err(format!("{at}'s {dep} isn't a version or a table"));
            };
            for key in [
                "git",
                "path",
                "registry",
                "registry-index",
                "branch",
                "tag",
                "rev",
                "base",
            ] {
                if spec.contains_key(key) {
                    return Err(format!("{at}'s {dep} comes from its {key}, not crates.io"));
                }
            }
        }
    }
    Ok(name.to_string())
}

/// How many crates `Cargo.lock` lists, or why its crates aren't fetched:
/// each from crates.io with its checksum, but the package itself (`own`),
/// which has neither. Read only as cargo writes it (a version line, then
/// `[[package]]` tables of plain `name`, `version`, `source`, `checksum` and
/// `dependencies` lines), so what this reads is what cargo reads: a line of
/// any other form is refused rather than parsed.
pub fn lock_problem(lock: &str, own: &str) -> Result<usize, String> {
    #[derive(Default)]
    struct Package {
        name: Option<String>,
        source: Option<String>,
        checksum: Option<String>,
        line: usize,
    }
    let plain = |v: &str| {
        v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+:/@ ()#?=&%~-".contains(&b))
    };
    let string = |line: &str, key: &str| -> Option<String> {
        let value = line
            .strip_prefix(key)?
            .strip_prefix(" = \"")?
            .strip_suffix('"')?;
        plain(value).then(|| value.to_string())
    };
    let mut packages: Vec<Package> = Vec::new();
    let mut in_list = false;
    for (n, line) in lock.lines().enumerate() {
        let at = n + 1;
        let odd = || format!("Cargo.lock's line {at} isn't as cargo writes it: {line}");
        if in_list {
            if line == "]" {
                in_list = false;
            } else if !line
                .strip_prefix(" \"")
                .and_then(|l| l.strip_suffix("\","))
                .is_some_and(plain)
            {
                return Err(odd());
            }
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[[package]]" {
            packages.push(Package {
                line: at,
                ..Package::default()
            });
            continue;
        }
        let Some(package) = packages.last_mut() else {
            // Before the first package, only the format's version.
            if line
                .strip_prefix("version = ")
                .is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
            {
                continue;
            }
            return Err(odd());
        };
        let slot = if let Some(v) = string(line, "name") {
            (&mut package.name, v)
        } else if let Some(v) = string(line, "source") {
            (&mut package.source, v)
        } else if let Some(v) = string(line, "checksum") {
            if v.len() != 64 || !v.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(odd());
            }
            (&mut package.checksum, v)
        } else if string(line, "version").is_some() {
            continue;
        } else if line == "dependencies = [" {
            in_list = true;
            continue;
        } else {
            return Err(odd());
        };
        if slot.0.replace(slot.1).is_some() {
            return Err(odd());
        }
    }
    if in_list {
        return Err("Cargo.lock ends inside a list".into());
    }
    for p in &packages {
        let name = p.name.as_deref().unwrap_or("");
        match (&p.source, &p.checksum) {
            (None, None) if name == own => {}
            (None, _) => {
                return Err(format!(
                    "Cargo.lock's {name} (line {}) isn't from a registry: a path dependency",
                    p.line
                ));
            }
            (Some(source), _) if !CRATES_IO.contains(&source.as_str()) => {
                return Err(format!(
                    "Cargo.lock's {name} (line {}) comes from {source}, not crates.io",
                    p.line
                ));
            }
            (Some(_), None) => {
                return Err(format!(
                    "Cargo.lock's {name} (line {}) has no checksum",
                    p.line
                ));
            }
            _ => {}
        }
    }
    Ok(packages.len())
}

// ── The try ─────────────────────────────────────────────────────────────────

/// A sealed try: your home, its home, the worktree it tries, and where it
/// all is as the sandbox names it.
struct Sealed {
    /// Your home's real path.
    real_real: PathBuf,
    home: PathBuf,
    /// The home's real path, whether or not it is there: `/private/tmp/...`
    /// on a Mac.
    home_real: PathBuf,
    tmp_real: PathBuf,
    wt: PathBuf,
    git_common: PathBuf,
    main_clone: PathBuf,
    cargo: PathBuf,
    rustc: PathBuf,
    rustdoc: Option<PathBuf>,
    toolchain: PathBuf,
    user_dir: Option<PathBuf>,
    build_path: Vec<PathBuf>,
    build_env: Vec<(String, String)>,
}

/// A tool of the toolchain your main clone builds with, as `rustup which`
/// names it there (that clone's `rust-toolchain.toml` picking it, never the
/// worktree's), else the one on your PATH, with its links followed. A
/// rustup proxy (which reads the folder's toolchain file, and `~/.rustup`)
/// is no answer.
fn tool(main_clone: &Path, name: &str) -> Option<PathBuf> {
    let mut rustup = Command::new("rustup");
    rustup.args(["which", name]).current_dir(main_clone);
    let found = util::run_command(&mut rustup)
        .ok()
        .map(|p| PathBuf::from(p.trim()))
        .or_else(|| util::which(name))?
        .canonicalize()
        .ok()?;
    (found.file_name().is_some_and(|n| n != "rustup")).then_some(found)
}

impl Sealed {
    fn new(real: PathBuf, home: PathBuf, wt: PathBuf) -> Result<Sealed> {
        let real_real = real.canonicalize().unwrap_or_else(|_| real.clone());
        let tmp_real = Path::new(try_::ROOT)
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(try_::ROOT));
        let home_real = tmp_real.join(name_of(&home));
        let common = try_::git(&wt, &["rev-parse", "--git-common-dir"])
            .ok_or_else(|| anyhow!("{} has no git directory", wt.display()))?;
        let git_common = wt
            .join(common)
            .canonicalize()
            .context("its git directory can't be read")?;
        let main_clone = git_common
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| anyhow!("{} has no main clone", git_common.display()))?;
        let cargo = tool(&main_clone, "cargo").ok_or_else(|| {
            anyhow!("couldn't find the toolchain's cargo (rustup which cargo, in your main clone)")
        })?;
        let rustc = tool(&main_clone, "rustc")
            .ok_or_else(|| anyhow!("couldn't find the toolchain's rustc"))?;
        let rustdoc = tool(&main_clone, "rustdoc");
        let toolchain = cargo
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .ok_or_else(|| anyhow!("{} isn't in a toolchain", cargo.display()))?;
        let (build_path, build_env) = platform::sealed_build();
        Ok(Sealed {
            real_real,
            home,
            home_real,
            tmp_real,
            wt,
            git_common,
            main_clone,
            cargo,
            rustc,
            rustdoc,
            toolchain,
            user_dir: platform::user_dir(),
            build_path,
            build_env,
        })
    }

    fn rules(&self, run: bool) -> sandbox::Rules {
        sandbox::Rules {
            home: self.home_real.clone(),
            real_home: self.real_real.clone(),
            user_dir: self.user_dir.clone(),
            tmp: self.tmp_real.clone(),
            // SAFETY: getuid only reads the process's user id.
            uid: unsafe { libc::getuid() },
            readable: vec![
                self.wt.clone(),
                self.git_common.clone(),
                self.toolchain.clone(),
            ],
            unwritable: vec![self.wt.clone(), self.git_common.clone()],
            run,
        }
    }

    /// Writes the rules' files into your cache, which marks the home as
    /// sealed before anything is made in it.
    fn write_rules(&self) -> Result<()> {
        let dir = sealed_dir();
        fs::create_dir_all(&dir).with_context(|| format!("could not make {}", dir.display()))?;
        let exe = std::env::current_exe()?.canonicalize()?;
        for (file, text) in [
            (
                profile_file(&self.home),
                sandbox::profile(&self.rules(false)),
            ),
            (
                run_profile_file(&self.home),
                sandbox::profile(&self.rules(true)),
            ),
            (
                client_profile_file(&self.home),
                sandbox::client(
                    &self.home_real,
                    &self.real_real,
                    self.user_dir.as_deref(),
                    &exe,
                ),
            ),
        ] {
            util::write_atomic(&file, text)
                .with_context(|| format!("could not write {}", file.display()))?;
        }
        Ok(())
    }

    /// The try's binary: the build's, as its link in the home names it.
    fn bin(&self) -> PathBuf {
        self.home.join(".local/bin/switchboard")
    }

    /// Where the try's first tab starts: a made-up repository, so `M-d` has
    /// a branch to show, else its home.
    fn start_dir(&self) -> PathBuf {
        let repo = self.home.join("code/acme-api");
        if repo.is_dir() {
            repo
        } else {
            self.home.clone()
        }
    }

    /// The PATH inside: the build first, then the toolchain, the C
    /// compiler's and git's own folders, then the system's.
    fn path(&self) -> String {
        let mut parts = vec![self.home.join(".local/bin"), self.toolchain.join("bin")];
        parts.extend(self.build_path.iter().cloned());
        parts.extend(
            [
                "/opt/homebrew/bin",
                "/usr/local/bin",
                "/usr/bin",
                "/bin",
                "/usr/sbin",
                "/sbin",
            ]
            .map(PathBuf::from),
        );
        parts
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(":")
    }

    /// The environment inside, made from nothing.
    fn env(&self) -> Vec<(String, String)> {
        let home = self.home.to_string_lossy().into_owned();
        let user = std::env::var("USER").unwrap_or_else(|_| "try".into());
        let mut env = vec![
            ("HOME".to_string(), home.clone()),
            ("USER".to_string(), user.clone()),
            ("LOGNAME".to_string(), user),
            ("SHELL".to_string(), "/bin/zsh".to_string()),
            ("PATH".to_string(), self.path()),
            (
                "TERM".to_string(),
                std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".into()),
            ),
            ("LANG".to_string(), "en_US.UTF-8".to_string()),
            ("LC_ALL".to_string(), "en_US.UTF-8".to_string()),
            ("TMPDIR".to_string(), format!("{home}/tmp")),
            ("SWITCHBOARD_SEALED".to_string(), "1".to_string()),
            ("CARGO_HOME".to_string(), format!("{home}/.cargo")),
            ("CARGO_TARGET_DIR".to_string(), format!("{home}/target")),
            ("CARGO_NET_OFFLINE".to_string(), "true".to_string()),
            (
                "RUSTC".to_string(),
                self.rustc.to_string_lossy().into_owned(),
            ),
        ];
        if let Ok(color) = std::env::var("COLORTERM") {
            env.push(("COLORTERM".to_string(), color));
        }
        if let Some(rustdoc) = &self.rustdoc {
            env.push((
                "RUSTDOC".to_string(),
                rustdoc.to_string_lossy().into_owned(),
            ));
        }
        env.extend(self.build_env.iter().cloned());
        env
    }

    /// `program` run in the sandbox under the rules in `profile`, with the
    /// environment inside and nothing of yours.
    fn command(&self, profile: &Path, program: &Path) -> Result<Command> {
        let words = platform::sandboxed(profile, program).map_err(|e| anyhow!(e))?;
        let (first, rest) = words
            .split_first()
            .ok_or_else(|| anyhow!("no sandbox to run in"))?;
        let mut command = Command::new(first);
        command.args(rest).env_clear().envs(self.env());
        Ok(command)
    }

    /// The command line that runs the try's binary in the sandbox, as a
    /// shell reads it: for `--print` and `--detached`'s prefix.
    fn words(&self) -> Result<String> {
        let mut words = vec!["/usr/bin/env".to_string(), "-i".to_string()];
        words.extend(
            self.env()
                .into_iter()
                .map(|(k, v)| format!("{k}={}", shell_quote(&v))),
        );
        let sandboxed =
            platform::sandboxed(&profile_file(&self.home), &self.bin()).map_err(|e| anyhow!(e))?;
        words.extend(sandboxed.iter().map(|w| shell_quote(w)));
        Ok(words.join(" "))
    }

    /// Whether the try is running: its server's socket is there, a socket
    /// at its own real path (not a link the try's code put there to your
    /// server's), and something listens on it. Connecting sends nothing.
    fn running(&self) -> bool {
        let socket = try_::socket_of(&self.home);
        let real = try_::socket_of(&self.home_real);
        socket
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_socket())
            && socket.canonicalize().ok() == Some(real)
            && UnixStream::connect(&socket).is_ok()
    }

    /// The home made from nothing, beside where it goes, then moved into
    /// place whole.
    fn make_home(&self, fetch: bool) -> Result<()> {
        let stage = staging(&self.home);
        remove(&stage);
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&stage)
            .with_context(|| format!("could not make {}", stage.display()))?;
        if let Err(e) = self
            .lay_out(&stage)
            .and_then(|_| self.crates(&stage, fetch))
        {
            remove(&stage);
            return Err(e);
        }
        fs::rename(&stage, &self.home).map_err(|e| {
            remove(&stage);
            anyhow!(
                "could not put {} in place ({e}); something made it again meanwhile: run switchboard try --clean {}",
                self.home.display(),
                shell_quote(&self.wt.to_string_lossy())
            )
        })
    }

    /// What the home holds before any of the try's code runs, in `stage`,
    /// every path inside written as it will be in the home.
    fn lay_out(&self, stage: &Path) -> Result<()> {
        for dir in [
            ".config/switchboard",
            ".local/bin",
            ".local/share",
            ".local/state/switchboard",
            ".claude",
            ".cargo",
            "tmp",
            "code",
            "Notes",
        ] {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(stage.join(dir))?;
        }
        symlink(&self.wt, stage.join("switchboard"))?;
        // The build is reached by its link, made now, since nothing may be
        // made in the home once the build has run.
        symlink(
            self.home.join("target/debug/switchboard"),
            stage.join(".local/bin/switchboard"),
        )?;
        let mut settings = Map::new();
        for (key, value) in SEALED_OFF {
            settings.insert(
                key.to_string(),
                serde_json::from_str(value).unwrap_or(json!(false)),
            );
        }
        util::write_atomic(
            stage.join(".config/switchboard/settings.json"),
            format!("{}\n", util::json_pretty(&Value::Object(settings))),
        )?;
        util::write_atomic(
            stage.join(".zshrc"),
            "# A sealed try's own: nothing of yours.\n",
        )?;
        util::write_atomic(
            stage.join(".gitconfig"),
            "[user]\n\tname = Try\n\temail = try@example.com\n",
        )?;
        util::write_atomic(
            stage.join("Notes/Getting started.md"),
            "# Getting started\n\nA made-up note in a sealed try.\n\n- [ ] Open the sidebar with M-s\n- [ ] Open mission control with M-Space\n\n- [x] Start the try\n",
        )?;
        util::write_atomic(
            stage.join("Notes/Groceries.md"),
            "# Groceries\n\n- [ ] Coffee\n- [ ] Lemons\n- [ ] Bread\n",
        )?;
        for name in ["acme-api", "acme-web"] {
            if let Err(e) = self.made_up(stage, name) {
                eprintln!("switchboard try: could not make the made-up repository {name} ({e:#})");
            }
        }
        Ok(())
    }

    /// A made-up repository in `~/code`, on main with a commit, its origin
    /// beside it in the home (so `M-d` finds main as the base), and a
    /// branch with one more. Made with no git config of yours: no template,
    /// hook or signing of yours runs.
    fn made_up(&self, stage: &Path, name: &str) -> Result<()> {
        let repo = stage.join("code").join(name);
        let origin = stage.join("code/.origin").join(format!("{name}.git"));
        let final_origin = self.home.join("code/.origin").join(format!("{name}.git"));
        let git = |dir: &Path, args: &[&str]| -> Result<()> {
            let status = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args([
                    "-c",
                    "core.hooksPath=/dev/null",
                    "-c",
                    "commit.gpgsign=false",
                    "-c",
                    "user.name=Try",
                    "-c",
                    "user.email=try@example.com",
                ])
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?;
            if !status.success() {
                bail!("git {} failed", args.first().unwrap_or(&""));
            }
            Ok(())
        };
        fs::create_dir_all(&repo)?;
        fs::create_dir_all(&origin)?;
        git(&repo, &["init", "-q"])?;
        git(&repo, &["symbolic-ref", "HEAD", "refs/heads/main"])?;
        git(&origin, &["init", "-q", "--bare"])?;
        git(&origin, &["symbolic-ref", "HEAD", "refs/heads/main"])?;
        fs::write(
            repo.join("README.md"),
            format!("# {name}\n\nA made-up repository.\n"),
        )?;
        fs::write(
            repo.join("index.js"),
            "export const hello = (name) => `Hello, ${name}`;\n",
        )?;
        git(&repo, &["add", "-A"])?;
        git(&repo, &["commit", "-q", "-m", &format!("Start {name}")])?;
        git(
            &repo,
            &["remote", "add", "origin", &origin.to_string_lossy()],
        )?;
        git(&repo, &["push", "-q", "-u", "origin", "main"])?;
        git(
            &repo,
            &[
                "remote",
                "set-url",
                "origin",
                &final_origin.to_string_lossy(),
            ],
        )?;
        git(&repo, &["remote", "set-head", "origin", "main"])?;
        git(&repo, &["checkout", "-q", "-b", "feature/greeting"])?;
        fs::write(
            repo.join("index.js"),
            "export const hello = (name) => `Hello, ${name}!`;\nexport const bye = (name) => `Bye, ${name}`;\n",
        )?;
        git(&repo, &["commit", "-q", "-am", "Say goodbye too"])?;
        Ok(())
    }

    /// The crates the build needs, fetched outside the sandbox only from
    /// crates.io with their checksums, then copied into `stage`'s cargo.
    fn crates(&self, stage: &Path, fetch: bool) -> Result<()> {
        let manifest = util::read_text(self.wt.join("Cargo.toml"))
            .ok_or_else(|| anyhow!("{} has no Cargo.toml", self.wt.display()))?;
        let lock = util::read_text(self.wt.join("Cargo.lock"))
            .ok_or_else(|| anyhow!("{} has no Cargo.lock", self.wt.display()))?;
        let own = manifest_problem(&manifest)
            .map_err(|e| anyhow!("{e}, so its crates aren't fetched"))?;
        let count =
            lock_problem(&lock, &own).map_err(|e| anyhow!("{e}, so its crates aren't fetched"))?;
        let yours = util::read_text(self.main_clone.join("Cargo.lock"));
        if yours.as_deref() != Some(lock.as_str()) && !fetch {
            bail!(
                "its Cargo.lock differs from your clone's, so its crates aren't fetched: read the change (switchboard pr-risk lists it), then run again with --fetch, which fetches them from crates.io with their checksums"
            );
        }
        let cache = sealed_dir();
        let cargo_home = cache.join("cargo");
        let empty = cache.join("empty");
        fs::create_dir_all(&cargo_home)?;
        remove(&empty);
        fs::create_dir_all(&empty)?;
        eprintln!("Fetching its {count} crates from crates.io…");
        requests::count(
            "crates.io",
            "cargo fetch",
            "switchboard try --sealed: the crates",
        );
        let mut command = Command::new(&self.cargo);
        command
            .args(["fetch", "--locked", "--manifest-path"])
            .arg(self.wt.join("Cargo.toml"))
            .current_dir(&empty)
            .env_clear()
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.toolchain.join("bin").display()),
            )
            .env("HOME", &empty)
            .env("CARGO_HOME", &cargo_home)
            .env("RUSTC", &self.rustc)
            .stdin(Stdio::null())
            .stdout(std::io::stderr())
            .stderr(Stdio::inherit());
        // A proxy you reach the internet through, and nothing else of yours.
        for (key, value) in std::env::vars() {
            let lower = key.to_ascii_lowercase();
            if matches!(
                lower.as_str(),
                "https_proxy" | "http_proxy" | "all_proxy" | "no_proxy"
            ) {
                command.env(key, value);
            }
        }
        let status = command.status().context("couldn't run cargo fetch")?;
        if !status.success() {
            bail!("cargo fetch failed for {}", self.wt.display());
        }
        for part in ["index", "cache"] {
            let from = cargo_home.join("registry").join(part);
            if from.is_dir() {
                let to = stage.join(".cargo/registry").join(part);
                fs::create_dir_all(to.parent().unwrap_or(stage))?;
                try_::copy_tree(&from, &to)?;
            }
        }
        Ok(())
    }

    /// The worktree built in the sandbox, offline and locked, into the
    /// home's `target`, its progress shown as it comes; then the build's own
    /// setup there, but its rules' repository, which it would fetch.
    fn build(&self) -> Result<()> {
        eprintln!("Building {} sealed…", self.wt.display());
        let mut cargo = self.command(&profile_file(&self.home), &self.cargo)?;
        cargo
            .args(["build", "--offline", "--locked"])
            .current_dir(&self.wt)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = cargo.spawn().context("couldn't run cargo in the sandbox")?;
        // What its build scripts and the compiler say of its files, shown
        // as it comes with their control characters written out, as
        // pr-risk shows the PR's lines.
        if let Some(said) = child.stderr.take() {
            use std::io::BufRead;
            for line in std::io::BufReader::new(said)
                .split(b'\n')
                .map_while(Result::ok)
            {
                eprintln!("{}", super::pr_risk::shown(&String::from_utf8_lossy(&line)));
            }
        }
        let status = child.wait().context("couldn't run cargo in the sandbox")?;
        if !status.success() {
            bail!(
                "cargo build failed in the sandbox (a toolchain other than your main clone's, or a crate it can't reach, say)"
            );
        }
        let out = self
            .command(&profile_file(&self.home), &self.bin())?
            .args(["setup", "plugin", "claude", "skills"])
            .current_dir(&self.home)
            .stdin(Stdio::null())
            .output();
        match out {
            Ok(out) => {
                for line in String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .chain(String::from_utf8_lossy(&out.stderr).lines())
                {
                    eprintln!("  {}", super::pr_risk::shown(line));
                }
                if !out.status.success() {
                    eprintln!(
                        "switchboard try: its setup reported a failure above; starting anyway."
                    );
                }
            }
            Err(e) => eprintln!("switchboard try: couldn't run its setup ({e}); starting anyway."),
        }
        Ok(())
    }

    /// Its server started in the sandbox with no client, its log written
    /// by the sandbox's own shell, then a session made in it by
    /// Switchboard's own binary in a sandbox of its own.
    fn start_detached(&self) -> Result<()> {
        if self.running() {
            return Ok(());
        }
        let mut command = self.command(&profile_file(&self.home), Path::new("/bin/sh"))?;
        command
            .args([
                "-c",
                "exec \"$0\" server >>\"$HOME/.local/state/switchboard/server.log\" 2>&1",
            ])
            .arg(self.bin())
            .current_dir(self.start_dir())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // SAFETY: setsid is async-signal-safe; a child just forked is no
        // process group leader, so it can't fail for that.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        command.spawn().context("couldn't start its server")?;
        let deadline = Instant::now() + Duration::from_secs(20);
        while !self.running() {
            if Instant::now() > deadline {
                bail!(
                    "its server didn't start; its log is {}",
                    self.home
                        .join(".local/state/switchboard/server.log")
                        .display()
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        call(&self.home, "session")
    }
}

// ── Starting one ────────────────────────────────────────────────────────────

/// `switchboard try --sealed`: the worktree (the one you are in, or
/// `target`) tried in the sandbox; with `run`, that command run there.
pub(crate) fn start(
    mode: Mode,
    target: Option<&Path>,
    fetch: bool,
    run: &[String],
) -> Result<ExitCode> {
    platform::sandbox_ready().map_err(|e| anyhow!(e))?;
    let from = match target {
        Some(dir) => dir.to_path_buf(),
        None => std::env::current_dir().context("can't read the directory you are in")?,
    };
    let top = try_::git(&from, &["rev-parse", "--show-toplevel"]).ok_or_else(|| {
        anyhow!("not in a git repository: run it in a Switchboard worktree, or name one")
    })?;
    let wt = PathBuf::from(top)
        .canonicalize()
        .context("the worktree can't be read")?;
    if !try_::is_switchboard(&wt) {
        bail!("{} isn't a Switchboard clone", wt.display());
    }
    if mode == Mode::Attach
        && run.is_empty()
        && std::env::var_os("SWITCHBOARD").is_some_and(|s| !s.is_empty())
    {
        bail!(
            "you're in one of Switchboard's panes, whose workspace would take every key of the try's; run it in a terminal of its own, or use --print there, or --detached to drive it with commands"
        );
    }
    let real = paths::home();
    // A PR wtpr --untrusted checked out (a worktree of a clone of yours,
    // its mark in its own config), one of your own, or one listed.
    let checked_out = trust::is_untrusted(&wt) && trust::leads_back(&wt);
    if !checked_out {
        trust::yours_or_listed(&real, &wt).map_err(|why| {
            anyhow!("{why}; or check it out with wtpr --untrusted, which try --sealed runs")
        })?;
    }
    let home = try_::home_of(Path::new(try_::ROOT), &wt);
    if crate::setup::exists(&home) && !is_sealed(&home) {
        bail!(
            "{} was made without --sealed, linking your files; run switchboard try --clean {} first",
            home.display(),
            shell_quote(&wt.to_string_lossy())
        );
    }
    let sealed = Sealed::new(real, home.clone(), wt.clone())?;
    let running = sealed.running();
    let digest = lock_digest(&wt);
    // A `--run` again on the same worktree takes up the home the last one
    // made, its crates fetched for the same lock, rather than fetching and
    // building them all again.
    let reusable = !run.is_empty()
        && crate::setup::exists(&home)
        && is_sealed(&home)
        && digest.is_some()
        && util::read_text(crates_file(&home)) == digest;
    if running || reusable {
        sealed.write_rules()?;
    } else {
        if crate::setup::exists(&home) || is_sealed(&home) {
            clean(&home)?;
        }
        sealed.write_rules()?;
        eprintln!("Setting up {} from nothing, sealed…", home.display());
        sealed.make_home(fetch)?;
        if let Some(digest) = &digest {
            util::write_atomic(crates_file(&home), digest)?;
        }
    }
    if let Some((program, args)) = run.split_first() {
        let mut command = sealed.command(&run_profile_file(&home), Path::new(program))?;
        command.args(args).current_dir(&wt);
        let e = command.exec();
        bail!("couldn't run {program} in the sandbox: {e}");
    }
    if !running {
        sealed.build()?;
    }
    match mode {
        Mode::Print => {
            println!("{}", sealed.words()?);
            eprintln!(
                "Run that in another terminal to attach to the try, sealed; switchboard try --clean {} removes it.",
                shell_quote(&wt.to_string_lossy())
            );
            Ok(ExitCode::SUCCESS)
        }
        Mode::Detached => {
            sealed.start_detached()?;
            let prefix = sealed.words()?;
            println!("Its server is running sealed, with no client. Drive and read it with:");
            println!("  {prefix} send-keys -l 'some text' && {prefix} send-keys Enter");
            println!("  {prefix} capture-pane");
            println!("  {prefix} list");
            println!(
                "switchboard try --clean {} ends it.",
                shell_quote(&wt.to_string_lossy())
            );
            Ok(ExitCode::SUCCESS)
        }
        _ => {
            let mut command = sealed.command(&profile_file(&home), &sealed.bin())?;
            command.current_dir(sealed.start_dir());
            let e = command.exec();
            bail!("couldn't run the try's switchboard in the sandbox: {e}")
        }
    }
}

// ── Reaching its server, and cleaning up ────────────────────────────────────

/// Has Switchboard's own binary, in the sandbox that reaches the try's
/// socket alone (`sandbox::client`), make a session in its server
/// (`session`) or stop it (`stop`).
fn call(home: &Path, what: &str) -> Result<()> {
    let exe = std::env::current_exe()?.canonicalize()?;
    let words = platform::sandboxed(&client_profile_file(home), &exe).map_err(|e| anyhow!(e))?;
    let (first, rest) = words
        .split_first()
        .ok_or_else(|| anyhow!("no sandbox to run in"))?;
    let out = Command::new(first)
        .args(rest)
        .args(["try", "--sealed-call", what])
        .arg(try_::socket_of(home))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .output()
        .context("couldn't reach the try's server")?;
    if !out.status.success() {
        bail!(
            "{}",
            super::pr_risk::shown(String::from_utf8_lossy(&out.stderr).trim())
        );
    }
    Ok(())
}

/// `try --sealed-call <session|stop> <socket>`, which `call` runs in its
/// sandbox: a session made in the server on that socket, as `tmux new -d`
/// makes one, or the server stopped.
pub fn call_main(args: &[String]) -> ExitCode {
    let result = match args {
        [what, socket] if what == "session" => try_::make_session(Path::new(socket)),
        [what, socket] if what == "stop" => {
            try_::stop_server(Path::new(socket));
            Ok(())
        }
        _ => Err(anyhow!(
            "usage: switchboard try --sealed-call <session|stop> <socket>"
        )),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e:#}");
            ExitCode::FAILURE
        }
    }
}

/// Stops a sealed try's server, through `call`, and removes its home (a
/// link put in its place removed as a link), a home half made, and its
/// rules.
pub fn clean(home: &Path) -> Result<()> {
    if platform::sandbox_ready().is_ok() && client_profile_file(home).exists() {
        let _ = call(home, "stop");
    }
    remove(home);
    remove(&staging(home));
    if crate::setup::exists(home) {
        bail!("could not remove {}", home.display());
    }
    for file in [
        profile_file(home),
        run_profile_file(home),
        client_profile_file(home),
        crates_file(home),
    ] {
        let _ = fs::remove_file(file);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn its_settings_turn_off_everything_that_reaches_a_service_or_claude() {
        let schema = crate::settings::schema();
        for (key, value) in SEALED_OFF {
            let option = schema.iter().find(|o| o.key == *key).expect("a setting");
            let value: Value = serde_json::from_str(value).unwrap();
            assert!(option.valid(&value), "{key} takes {value}");
        }
    }

    #[test]
    fn only_crates_io_dependencies_are_fetched() {
        assert_eq!(
            manifest_problem(
                "[package]\nname = \"switchboard\"\n[dependencies]\nregex = \"1\"\nserde = { version = \"1\", features = [\"derive\"] }\n[target.'cfg(target_os = \"linux\")'.dependencies]\nzbus = { version = \"6\", default-features = false }\n[dev-dependencies]\n"
            ),
            Ok("switchboard".to_string())
        );
        for (manifest, why) in [
            (
                "[package]\nname = \"x\"\n[dependencies]\ny = { git = \"https://evil.example/y\" }\n",
                "comes from its git",
            ),
            (
                "[package]\nname = \"x\"\n[build-dependencies]\ny = { path = \"../y\" }\n",
                "comes from its path",
            ),
            (
                "[package]\nname = \"x\"\n[target.'cfg(unix)'.dev-dependencies]\ny = { version = \"1\", registry = \"mine\" }\n",
                "comes from its registry",
            ),
            (
                "[package]\nname = \"x\"\n[patch.crates-io]\ny = { git = \"https://evil.example/y\" }\n",
                "[patch]",
            ),
            (
                "\"patch\" = { crates-io = { y = { path = \"/y\" } } }\n[package]\nname = \"x\"\n",
                "[patch]",
            ),
            (
                "[package]\nname = \"x\"\n[replace]\n\"y:1.0.0\" = { path = \"/y\" }\n",
                "[replace]",
            ),
            (
                "[package]\nname = \"x\"\nworkspace = \"..\"\n",
                "workspace elsewhere",
            ),
            (
                "[package]\nname = \"x\"\n[workspace]\nmembers = [\"y\"]\n",
                "several packages",
            ),
            (
                "[package]\nname = \"x\"\n[workspace.dependencies]\ny = { git = \"https://evil.example/y\" }\n",
                "comes from its git",
            ),
            ("[dependencies]\n", "names no package"),
            ("[package\n", "doesn't read as TOML"),
        ] {
            let got = manifest_problem(manifest).unwrap_err();
            assert!(got.contains(why), "{manifest}: {got}");
        }
    }

    #[test]
    fn the_lockfile_is_read_only_as_cargo_writes_it() {
        let sum = "a".repeat(64);
        let good = format!(
            "# This file is automatically @generated by Cargo.\n# It is not intended for manual editing.\nversion = 4\n\n[[package]]\nname = \"regex\"\nversion = \"1.12.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"{sum}\"\ndependencies = [\n \"aho-corasick\",\n \"memchr 2.7.4 (registry+https://github.com/rust-lang/crates.io-index)\",\n]\n\n[[package]]\nname = \"switchboard\"\nversion = \"0.1.1\"\ndependencies = [\n \"regex\",\n]\n"
        );
        assert_eq!(lock_problem(&good, "switchboard"), Ok(2));
        // The repository's own lockfile reads.
        let ours =
            std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.lock"))
                .unwrap();
        assert!(lock_problem(&ours, "switchboard").is_ok());

        for (lock, why) in [
            (good.replace("registry+https://github.com/rust-lang/crates.io-index\"\nchecksum", "git+https://evil.example/r#abc\"\nchecksum"), "not crates.io"),
            (good.replace(&format!("checksum = \"{sum}\"\n"), ""), "no checksum"),
            (good.replace("name = \"switchboard\"", "name = \"other\""), "a path dependency"),
            (good.replace("version = \"1.12.0\"", "version = '1.12.0'"), "isn't as cargo writes it"),
            (good.replace("source = ", "source  = "), "isn't as cargo writes it"),
            (good.replace("[[package]]\nname = \"regex\"", "[[package]]\nname = \"regex\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\""), "isn't as cargo writes it"),
            (format!("{good}[[patch.unused]]\nname = \"y\"\n"), "isn't as cargo writes it"),
            (good.replace(" \"aho-corasick\",", " \"aho-corasick\" ,"), "isn't as cargo writes it"),
        ] {
            let got = lock_problem(&lock, "switchboard").unwrap_err();
            assert!(got.contains(why), "{got}");
        }
    }

    #[test]
    fn a_home_being_made_is_beside_it_and_its_rules_name_it() {
        let home = Path::new("/tmp/sb-0123abcd");
        assert_eq!(staging(home), PathBuf::from("/tmp/sb-0123abcd.new"));
        assert!(profile_file(home).ends_with("sealed/sb-0123abcd.sb"));
        assert!(run_profile_file(home).ends_with("sealed/sb-0123abcd-run.sb"));
    }

    /// The rules are named by the home's name alone, so a folder of that
    /// name outside the tries' root (a test's) is never taken for a sealed
    /// home, and cleaning it never removes a real try's rules.
    #[test]
    fn only_a_home_in_the_tries_root_has_rules() {
        assert!(in_root(Path::new("/tmp/sb-0123abcd")));
        assert!(!in_root(Path::new("/var/folders/x/sb-0123abcd")));
        assert!(!is_sealed(Path::new("/var/folders/x/sb-0123abcd")));
        assert!(homes(Path::new("/var/folders/x")).is_empty());
    }
}
