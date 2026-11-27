//! `switchboard clone-build <worktree>`: starts a new worktree's `target/`
//! as a copy-on-write clone of its main clone's, so a Rust worktree reuses
//! the dependencies the main clone built rather than compiling them all
//! again, and takes no disk space for them until a build changes one. New
//! with the port, after ten worktrees at 7 to 26 GB each filled a disk.
//!
//! The worktree commands (`wt`, `wtb`, `wtpr`, and autopilot through
//! `wt`) call `start` once they have made a worktree and found its branch
//! yours (or `--trusted`), which runs this in the background, detached, so Claude starts without waiting
//! on it. The clone is made beside the main clone's `.git` and renamed into
//! place only while the worktree still has no `target/`, so a build that
//! started first keeps its own and the clone is dropped. Cargo decides what
//! is stale: every file in the clone newer than the worktree is set back to
//! before it (`set_back`), so the worktree's files are newer than any record
//! of a build, its own crates build again, and the dependencies, keyed by
//! their version, are reused.
//!
//! It is skipped, saying nothing, with Settings' `worktree_clone_build`
//! off (its default, until it has been tried on real worktrees), `CARGO_TARGET_DIR`, `CARGO_BUILD_TARGET_DIR` or
//! `CARGO_BUILD_BUILD_DIR` set, or a cargo config naming a `target-dir` or
//! `build-dir` (cargo then builds elsewhere), in a worktree holding someone
//! else's code (`trust::is_untrusted`, or a branch with someone else's commits,
//! `--trusted` or not), one whose `target/` git doesn't ignore, when the
//! main clone has no `target/` cargo made (one without its `CACHEDIR.TAG`
//! may be a project's own data) or the worktree has one, when the two aren't
//! on one volume, and where the filesystem can't clone (`platform::
//! clone_tree`); it never falls back to a plain copy.

use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use crate::{platform, settings, trust, worktree};

/// The setting that turns it off.
pub const SETTING: &str = "worktree_clone_build";

/// The main clone's `target/` to clone into `worktree` and the folder to
/// make the clone in before it is renamed into place, or `None` when it is
/// skipped (above).
pub fn source(worktree: &Path) -> Option<(PathBuf, PathBuf)> {
    // Only when set on, so a schema that doesn't list it (an older one, read
    // from a clone) never turns it on.
    if settings::setting(SETTING) != Some(serde_json::Value::Bool(true))
        || [
            "CARGO_TARGET_DIR",
            "CARGO_BUILD_TARGET_DIR",
            "CARGO_BUILD_BUILD_DIR",
        ]
        .iter()
        .any(|v| std::env::var_os(v).is_some())
    {
        return None;
    }
    plan(worktree)
}

/// Whether a cargo config file cargo reads for `dir` (one in `.cargo/` of
/// it or a folder above it, or in cargo's home) names a `target-dir` or a
/// `build-dir`, so builds go elsewhere than `target/`: its `[build]`
/// table's, read as TOML, from a regular file under a megabyte (a link
/// followed, as dotfiles link cargo's config).
fn target_dir_configured(dir: &Path) -> bool {
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::paths::home_join(".cargo"));
    target_dir_configured_in(dir, &cargo_home)
}

/// `target_dir_configured` with cargo's home given.
fn target_dir_configured_in(dir: &Path, cargo_home: &Path) -> bool {
    let names_one = |file: PathBuf| {
        let small_file = std::fs::metadata(&file).is_ok_and(|m| m.is_file() && m.len() < 1 << 20);
        small_file
            && std::fs::read_to_string(&file)
                .ok()
                .and_then(|text| text.parse::<toml::Table>().ok())
                .is_some_and(|config| {
                    config
                        .get("build")
                        .and_then(toml::Value::as_table)
                        .is_some_and(|build| {
                            build.contains_key("target-dir") || build.contains_key("build-dir")
                        })
                })
    };
    dir.ancestors()
        .map(|d| d.join(".cargo"))
        .chain([cargo_home.to_path_buf()])
        .flat_map(|d| [d.join("config.toml"), d.join("config")])
        .any(names_one)
}

/// `source`'s checks of the folders alone.
fn plan(worktree: &Path) -> Option<(PathBuf, PathBuf)> {
    if trust::is_untrusted(worktree) {
        return None;
    }
    let main = worktree::repo_root_of(worktree)?;
    let from = main.join("target");
    let git_dir = main.join(".git");
    let to = worktree.join("target");
    let same = |a: &Path, b: &Path| {
        let (Ok(a), Ok(b)) = (a.canonicalize(), b.canonicalize()) else {
            return false;
        };
        a == b
    };
    let from_meta = std::fs::symlink_metadata(&from).ok()?;
    let on_one_volume = std::fs::metadata(worktree).ok()?.dev() == from_meta.dev()
        && std::fs::metadata(&git_dir).ok()?.dev() == from_meta.dev();
    // A linked worktree's .git is a file naming its folder in the main
    // clone's, never a subfolder of a checkout.
    let linked = std::fs::symlink_metadata(worktree.join(".git")).is_ok_and(|m| m.is_file());
    let wanted = !same(&main, worktree)
        && linked
        && from_meta.is_dir()
        && made_by_cargo(&from)
        && git_dir.is_dir()
        && std::fs::symlink_metadata(&to).is_err()
        && on_one_volume;
    wanted.then(|| {
        let scratch = git_dir.join(format!("switchboard-target-clone-{}", std::process::id()));
        (from, scratch)
    })
}

/// Whether `target` is a folder cargo made: its `CACHEDIR.TAG`, a file,
/// says cargo created it, as cargo writes it.
fn made_by_cargo(target: &Path) -> bool {
    let tag = target.join("CACHEDIR.TAG");
    std::fs::symlink_metadata(&tag).is_ok_and(|m| m.is_file())
        && std::fs::read_to_string(&tag).is_ok_and(|t| {
            t.starts_with("Signature: 8a477f597d28d172789f06886806bc55")
                && t.contains("created by cargo")
        })
}

/// What the background step checks besides `source`, since each runs git
/// or reads the checkout's files: the worktree's branch holds no one else's
/// commits (`trust::others_on`, whatever `trusted-dirs` lists), no cargo
/// config names a `target-dir`, git ignores its `target/`, so the
/// clone never shows as untracked files, and the main clone tracks nothing
/// under its own `target/`, so all of it is build output; and the worktree
/// is a cargo workspace itself (its own `Cargo.toml`), since a branch that
/// isn't would never have cargo use, or tag, its `target/`.
fn wanted_in(worktree: &Path) -> bool {
    let tracks_none = |main: PathBuf| {
        worktree::git(&main, &["ls-files", "--", ":(icase)target/"]).is_ok_and(|f| f.is_empty())
    };
    worktree.join("Cargo.toml").is_file()
        && !trust::is_untrusted(worktree)
        && trust::midway_refusal(worktree).is_ok()
        && trust::others_on(worktree, "HEAD", None).is_ok_and(|others| others.is_empty())
        && !target_dir_configured(worktree)
        && worktree::git_ok(worktree, &["check-ignore", "-q", "target/"])
        && worktree::repo_root_of(worktree).is_some_and(tracks_none)
}

/// Sets back to `before` the modified time of every file in `dir` newer
/// than it, so no record of a build there is newer than the worktree's
/// files, which git wrote after `before`: cargo then builds the worktree's
/// own crates again, whatever the main clone built since, while the
/// dependencies, which cargo keys by their version rather than by times,
/// are reused. A link fails it, since one could point the clone at a
/// folder another checkout builds in.
fn set_back(dir: &Path, before: std::time::SystemTime) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(std::io::Error::other("a link in the build folder"));
        }
        if kind.is_dir() {
            set_back(&entry.path(), before)?;
        } else if kind.is_file() && entry.metadata()?.modified()? > before {
            std::fs::File::options()
                .write(true)
                .open(entry.path())?
                .set_modified(before)?;
        }
    }
    Ok(())
}

/// Keeps in the clone `target` only cargo's compiler cache: each profile's
/// `deps/`, `.fingerprint/` and `incremental/` (a profile being a folder
/// holding `.fingerprint/`, in `target/` or in a target's folder there), and cargo's `CACHEDIR.TAG`. Everything else goes: the
/// binaries and libraries a build ends with, so nothing reads the main
/// clone's as the worktree's (cargo puts them there again from `deps/`);
/// examples, debug symbols and rustdoc's pages; build scripts and their
/// output (`build/`), since cargo keeps what is in a script's folder when it
/// runs it again, so each runs once more in the worktree; and what other
/// tools keep in `target/` (Criterion's benchmark baselines), which belongs
/// to the checkout that made it.
fn keep_compiler_cache(target: &Path) -> std::io::Result<()> {
    // A build profile holds cargo's records of its builds; rustdoc's pages
    // and `cargo package`'s unpacked crates are never one, whatever folders
    // they hold.
    let is_profile = |dir: &Path| {
        let named = dir.file_name().and_then(|n| n.to_str());
        dir.join(".fingerprint").is_dir() && !matches!(named, Some("doc" | "package"))
    };
    let remove = |path: &Path, is_dir: bool| {
        if is_dir {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        }
    };
    let prune_profile = |profile: &Path| -> std::io::Result<()> {
        for entry in std::fs::read_dir(profile)? {
            let entry = entry?;
            let is_dir = entry.file_type()?.is_dir();
            let cache = ["deps", ".fingerprint", "incremental"]
                .iter()
                .any(|kept| entry.file_name() == *kept);
            if !(is_dir && cache) {
                remove(&entry.path(), is_dir)?;
            }
        }
        Ok(())
    };
    // Cargo's tag, which it writes in a target's folder too.
    let is_tag = |entry: &std::fs::DirEntry| entry.file_name() == "CACHEDIR.TAG";
    for entry in std::fs::read_dir(target)? {
        let entry = entry?;
        let path = entry.path();
        let is_dir = entry.file_type()?.is_dir();
        if !is_dir {
            if !is_tag(&entry) {
                remove(&path, false)?;
            }
            continue;
        }
        let not_cargos_cache = matches!(entry.file_name().to_str(), Some("doc" | "package"));
        if not_cargos_cache {
            remove(&path, true)?;
            continue;
        }
        if is_profile(&path) {
            prune_profile(&path)?;
            continue;
        }
        let holds_profiles = std::fs::read_dir(&path)?
            .flatten()
            .any(|inner| is_profile(&inner.path()));
        if !holds_profiles {
            remove(&path, true)?;
            continue;
        }
        for inner in std::fs::read_dir(&path)? {
            let inner = inner?;
            let inner_dir = inner.file_type()?.is_dir();
            if inner_dir && is_profile(&inner.path()) {
                prune_profile(&inner.path())?;
            } else if inner_dir || !is_tag(&inner) {
                remove(&inner.path(), inner_dir)?;
            }
        }
    }
    Ok(())
}

/// A second before the oldest of `worktree`'s files git tracks and its
/// `.git`, so every file of the worktree is newer than what `set_back`
/// leaves; `None` for a worktree made more than ten minutes ago, reopened
/// rather than new, whose files may be older than the dependencies' own
/// records of a build, which setting them back would have cargo build again.
fn made_at(worktree: &Path) -> Option<std::time::SystemTime> {
    let tracked = worktree::git(worktree, &["ls-files", "-z"]).ok()?;
    let oldest = tracked
        .split('\0')
        .filter(|f| !f.is_empty())
        .map(|f| worktree.join(f))
        .chain([worktree.join(".git")])
        .filter_map(|f| std::fs::symlink_metadata(f).ok()?.modified().ok())
        .min()?;
    let new = oldest
        .elapsed()
        .is_ok_and(|age| age < std::time::Duration::from_secs(600));
    new.then(|| oldest.checked_sub(std::time::Duration::from_secs(1)))?
}

/// Starts the clone for a worktree just made, in the background, when it
/// isn't skipped. Waits only for a process that starts the clone detached
/// and exits, so nothing is left for the caller to reap once it execs
/// Claude.
pub fn start(worktree: &Path) {
    if source(worktree).is_none() {
        return;
    }
    let Ok(exe) = crate::util::exe() else {
        return;
    };
    let _ = Command::new(exe)
        .args(["clone-build", "--detach"])
        .arg(worktree)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Clones `from` as `worktree`'s `target/`, through `scratch`, put in place
/// only while `still_wanted` holds; whether it is in place.
pub fn clone_into(
    from: &Path,
    scratch: &Path,
    worktree: &Path,
    still_wanted: impl Fn() -> bool,
) -> bool {
    let Some(before) = made_at(worktree) else {
        return false;
    };
    if platform::clone_tree(from, scratch).is_err() {
        return false;
    }
    if set_back(scratch, before).is_err() || keep_compiler_cache(scratch).is_err() {
        let _ = std::fs::remove_dir_all(scratch);
        return false;
    }
    let to = worktree.join("target");
    // Asked again as it goes in place: a merge started while it was made
    // may have brought in someone else's files.
    // Its folder made first, which fails when a build made one, and the
    // clone renamed over it while it is still empty: a build that comes
    // between writes into it, and the rename then fails and keeps the
    // build's.
    let made = still_wanted() && std::fs::create_dir(&to).is_ok();
    let placed = made && std::fs::rename(scratch, &to).is_ok();
    if made && !placed {
        // Only while still empty, never a build's files.
        let _ = std::fs::remove_dir(&to);
    }
    if !placed {
        let _ = std::fs::remove_dir_all(scratch);
    }
    placed
}

pub fn main(args: Vec<String>) -> ExitCode {
    let (detach, rest): (Vec<&String>, Vec<&String>) =
        args.iter().partition(|a| a.as_str() == "--detach");
    let [worktree] = rest.as_slice() else {
        eprintln!("usage: switchboard clone-build <worktree>");
        return ExitCode::from(2);
    };
    // Whole and real, so the cargo configs of the folders above it are
    // found as cargo finds them.
    let Ok(worktree) = std::fs::canonicalize(worktree.as_str()) else {
        return ExitCode::FAILURE;
    };
    let worktree = worktree.as_path();
    if !detach.is_empty() {
        let Ok(exe) = crate::util::exe() else {
            return ExitCode::FAILURE;
        };
        let mut command = Command::new(exe);
        command
            .arg("clone-build")
            .arg(worktree)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // SAFETY: setsid is async-signal-safe, so it may run between fork
        // and exec; it fails only for a process group leader, which a child
        // just forked is not.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        return match command.spawn() {
            Ok(_) => ExitCode::SUCCESS,
            Err(_) => ExitCode::FAILURE,
        };
    }
    match source(worktree) {
        Some((from, scratch))
            if wanted_in(worktree)
                && clone_into(&from, &scratch, worktree, || wanted_in(worktree)) =>
        {
            ExitCode::SUCCESS
        }
        _ => ExitCode::FAILURE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worktree::git_scratch;

    /// Removes its folder when the test ends.
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A repository with a commit and a worktree under its
    /// `.claude/worktrees`, in a temporary folder of the test's own.
    fn repo(name: &str) -> (Scratch, PathBuf, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("sb-clone-build-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let main = dir.join("main");
        std::fs::create_dir_all(&main).unwrap();
        let tmp = Scratch(dir);
        git_scratch(&main, &["init", "-q", "-b", "main"]);
        git_scratch(
            &main,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "one",
            ],
        );
        let wt = main.join(".claude/worktrees/wt");
        git_scratch(
            &main,
            &["worktree", "add", "-q", "-b", "wt", wt.to_str().unwrap()],
        );
        (tmp, main, wt)
    }

    /// The main clone's `target/` as cargo makes it, marked a cache.
    fn cargo_target(main: &Path) -> PathBuf {
        let target = main.join("target");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(
            target.join("CACHEDIR.TAG"),
            "Signature: 8a477f597d28d172789f06886806bc55\n# This file is a cache directory tag created by cargo.\n",
        )
        .unwrap();
        target
    }

    #[test]
    fn nothing_to_clone_without_the_main_clones_target() {
        let (_tmp, _main, wt) = repo("1");
        assert!(plan(&wt).is_none());
    }

    #[test]
    fn a_target_cargo_didnt_make_is_never_cloned() {
        let (_tmp, main, wt) = repo("6");
        std::fs::create_dir_all(main.join("target")).unwrap();
        std::fs::write(main.join("target/rows.db"), "data").unwrap();
        assert!(plan(&wt).is_none(), "a project's own data");
        cargo_target(&main);
        assert!(plan(&wt).is_some());
    }

    #[test]
    fn a_worktree_with_a_target_of_its_own_keeps_it() {
        let (_tmp, main, wt) = repo("2");
        cargo_target(&main);
        std::fs::create_dir(wt.join("target")).unwrap();
        assert!(plan(&wt).is_none());
    }

    #[test]
    fn the_main_clone_is_never_cloned_into_itself() {
        let (_tmp, main, _wt) = repo("3");
        cargo_target(&main);
        assert!(plan(&main).is_none());
    }

    /// On APFS the clone matches the main clone's files, times included, so
    /// cargo finds the worktree's newer sources stale, and the folder made
    /// for it beside `.git` is gone.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_worktree_gets_a_clone_of_the_main_clones_target() {
        let (_tmp, main, wt) = repo("4");
        let deps = cargo_target(&main).join("debug/deps");
        std::fs::create_dir_all(&deps).unwrap();
        std::fs::create_dir_all(main.join("target/debug/.fingerprint")).unwrap();
        let rlib = deps.join("libdep.rlib");
        std::fs::write(&rlib, vec![7u8; 1 << 20]).unwrap();
        std::fs::write(main.join("target/debug/switchboard"), "main's binary").unwrap();
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        std::fs::File::options()
            .write(true)
            .open(&rlib)
            .unwrap()
            .set_modified(old)
            .unwrap();

        // Built in the main clone after the worktree was made.
        let fresh = deps.join("libown.rlib");
        std::fs::write(&fresh, "own").unwrap();
        let made = std::fs::metadata(wt.join(".git"))
            .unwrap()
            .modified()
            .unwrap();
        std::fs::File::options()
            .write(true)
            .open(&fresh)
            .unwrap()
            .set_modified(made + std::time::Duration::from_secs(5))
            .unwrap();

        let (from, scratch) = plan(&wt).expect("a clone to make");
        assert!(clone_into(&from, &scratch, &wt, || true));
        let own = std::fs::metadata(wt.join("target/debug/deps/libown.rlib")).unwrap();
        assert!(
            own.modified().unwrap() < made,
            "set back before the worktree's files, so cargo builds its own crates again"
        );

        let cloned = wt.join("target/debug/deps/libdep.rlib");
        assert_eq!(std::fs::read(&cloned).unwrap(), vec![7u8; 1 << 20]);
        assert_eq!(std::fs::metadata(&cloned).unwrap().modified().unwrap(), old);
        assert!(
            !wt.join("target/debug/switchboard").exists(),
            "the main clone's binary is never taken for the worktree's"
        );
        assert!(!scratch.exists());
        assert!(plan(&wt).is_none(), "a second run leaves it alone");
    }

    #[test]
    fn a_build_folder_holding_a_link_is_never_cloned() {
        let (_tmp, main, wt) = repo("7");
        let target = cargo_target(&main);
        std::os::unix::fs::symlink(main.join("elsewhere"), target.join("debug")).unwrap();
        let (from, scratch) = plan(&wt).unwrap();
        assert!(!clone_into(&from, &scratch, &wt, || true));
        assert!(!wt.join("target").exists());
        assert!(!scratch.exists());
    }

    #[test]
    fn a_target_dir_in_cargos_config_is_read_as_toml() {
        let (_tmp, main, wt) = repo("8");
        let config = main.join(".cargo/config.toml");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(&config, "# target-dir = \"out\"\n").unwrap();
        assert!(!target_dir_configured_in(&wt, &main), "commented out");
        std::fs::write(&config, "build.target-dir = \"out\"\n").unwrap();
        assert!(target_dir_configured_in(&wt, &main), "a dotted key");
        std::fs::write(&config, "[build]\n\"target-dir\" = \"out\"\n").unwrap();
        assert!(target_dir_configured_in(&wt, &main), "a quoted key");
    }

    #[test]
    fn only_cargos_compiler_cache_is_kept_for_every_target() {
        let (_tmp, main, _wt) = repo("9");
        let target = cargo_target(&main);
        let triple = target.join("aarch64-apple-darwin");
        for dir in [
            "debug/deps",
            "debug/.fingerprint",
            "debug/incremental/own",
            "debug/build/dep-0123456789abcdef/out",
            "debug/switchboard.dSYM/Contents",
            "debug/examples",
            "aarch64-apple-darwin/release/deps",
            "aarch64-apple-darwin/release/.fingerprint",
            "aarch64-apple-darwin/doc/deps/.fingerprint",
            "package/crate-0.1.0/deps",
            "package/crate-0.1.0/.fingerprint",
            "x86_64-unknown-linux-gnu/release",
            "criterion/bench/base",
            "doc/dep",
        ] {
            std::fs::create_dir_all(target.join(dir)).unwrap();
        }
        std::fs::copy(target.join("CACHEDIR.TAG"), triple.join("CACHEDIR.TAG")).unwrap();
        for file in [
            "debug/switchboard",
            "debug/deps/libdep.rlib",
            "debug/examples/demo",
            "aarch64-apple-darwin/.rustc_info.json",
            "aarch64-apple-darwin/release/switchboard",
            "aarch64-apple-darwin/release/deps/libdep.rlib",
            "x86_64-unknown-linux-gnu/release/switchboard",
            "criterion/bench/base/estimates.json",
            ".rustc_info.json",
        ] {
            std::fs::write(target.join(file), "x").unwrap();
        }
        keep_compiler_cache(&target).unwrap();
        for gone in [
            "debug/switchboard",
            "debug/build",
            "debug/switchboard.dSYM",
            "debug/examples",
            "aarch64-apple-darwin/.rustc_info.json",
            "aarch64-apple-darwin/release/switchboard",
            "aarch64-apple-darwin/doc",
            "package",
            "x86_64-unknown-linux-gnu",
            "criterion",
            "doc",
            ".rustc_info.json",
        ] {
            assert!(!target.join(gone).exists(), "{gone}");
        }
        for kept in [
            "debug/deps/libdep.rlib",
            "debug/.fingerprint",
            "debug/incremental/own",
            "aarch64-apple-darwin/release/deps/libdep.rlib",
            "aarch64-apple-darwin/CACHEDIR.TAG",
            "CACHEDIR.TAG",
        ] {
            assert!(target.join(kept).exists(), "{kept}");
        }
    }

    #[test]
    fn a_target_made_while_cloning_is_kept() {
        let (_tmp, main, wt) = repo("5");
        std::fs::create_dir_all(cargo_target(&main).join("debug")).unwrap();
        let (from, scratch) = plan(&wt).unwrap();
        std::fs::create_dir_all(wt.join("target/own")).unwrap();
        assert!(!clone_into(&from, &scratch, &wt, || true));
        assert!(wt.join("target/own").is_dir());
        assert!(!scratch.exists());
    }
}
