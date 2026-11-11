//! M-p's "Restart Switchboard on the latest build on disk": what runs before
//! the restart (src/mux/upgrade.rs) so that it has something new to restart
//! on. A binary built from a clone knows the checkout and target directory
//! it was built in (build.rs's `SWITCHBOARD_CHECKOUT` and
//! `SWITCHBOARD_TARGET_DIR`), so it builds that checkout again with `cargo
//! build --release --locked` in that target directory, as `swb build` and
//! the README's install do, and puts the result in place of the binary the
//! server runs when that isn't the build's own file (a copy installed in
//! `~/.local/bin`). Cargo runs the checkout's own code, so it builds only
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
//! checkout to build (with cargo's own target directory), or, empty, none;
//! the tests use it so that nothing they do builds this repository.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

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

/// The checkout this binary was built from and the target directory it was
/// built in, as the module's comment says; `None` for a release build.
pub fn checkout() -> Option<(PathBuf, Option<PathBuf>)> {
    let nonempty = |s: &str| (!s.trim().is_empty()).then(|| PathBuf::from(s.trim()));
    if let Ok(given) = std::env::var("SWITCHBOARD_CHECKOUT") {
        return nonempty(&given).map(|dir| (dir, None));
    }
    let dir = nonempty(env!("SWITCHBOARD_CHECKOUT"))?;
    Some((dir, nonempty(env!("SWITCHBOARD_TARGET_DIR"))))
}

/// The `cargo` to build with: the PATH's, else the one the build had.
fn cargo() -> PathBuf {
    crate::util::which("cargo")
        .or_else(|| Some(PathBuf::from(env!("SWITCHBOARD_CARGO"))).filter(|p| p.is_file()))
        .unwrap_or_else(|| PathBuf::from("cargo"))
}

/// Builds `checkout` as the module's comment says and gives the binary it
/// made, or why it couldn't, in words.
pub fn build(checkout: &Path, target: Option<&Path>) -> Result<PathBuf, String> {
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
    crate::trust::yours_or_listed(&paths::home(), checkout)?;
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
    .stdin(Stdio::null());
    if let Some(target) = target {
        cmd.env("CARGO_TARGET_DIR", target);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("cargo couldn't be run ({e})"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        return Err(failure(&stderr));
    }
    built(&stdout).ok_or_else(|| "cargo didn't say where it put the binary".to_string())
}

/// The `switchboard` binary among the artifacts cargo's JSON messages name.
fn built(messages: &str) -> Option<PathBuf> {
    messages
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|m| m["reason"] == "compiler-artifact" && m["target"]["name"] == "switchboard")
        .filter_map(|m| m["executable"].as_str().map(PathBuf::from))
        .next_back()
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
    let built_in = checkout().and_then(|(_, target)| target);
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
) -> Result<Option<PathBuf>, String> {
    let to_build = checkout().filter(|_| build_first);
    if in_target_dir(binary) {
        let Some((dir, target)) = to_build else {
            return match installed(home) {
                Some(installed) => Ok(Some(installed)),
                None if !is_new(binary) => Ok(None),
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
                return Ok(Some(installed));
            }
        }
        let on = restart_on(home, binary);
        let built = build(&dir, target.as_deref())?;
        put(home, &built, &on)?;
        *last_built = Stamp::of(&on);
        return Ok(Some(on));
    }
    let mut last_built = LAST_BUILT.lock().unwrap_or_else(|e| e.into_inner());
    let now = Stamp::of(binary);
    let installed_by_hand = now.is_some() && is_new(binary) && *last_built != now;
    if installed_by_hand {
        return Ok(Some(binary.to_path_buf()));
    }
    if let Some((dir, target)) = to_build {
        let built = build(&dir, target.as_deref())?;
        install(&built, binary)?;
        *last_built = Stamp::of(binary);
    }
    Ok(is_new(binary).then(|| binary.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_switchboard_binary_among_cargos_messages() {
        let messages = [
            r#"{"reason":"compiler-artifact","target":{"name":"serde","kind":["lib"]},"executable":null}"#,
            "not json",
            r#"{"reason":"compiler-artifact","target":{"name":"switchboard","kind":["lib"]},"executable":null}"#,
            r#"{"reason":"compiler-artifact","target":{"name":"switchboard","kind":["bin"]},"executable":"/t/release/switchboard"}"#,
            r#"{"reason":"build-finished","success":true}"#,
        ]
        .join("\n");
        assert_eq!(
            built(&messages),
            Some(PathBuf::from("/t/release/switchboard"))
        );
        assert_eq!(built(r#"{"reason":"build-finished"}"#), None);
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
