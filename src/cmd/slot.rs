//! `switchboard slot <kind> -- <command>`: runs a command holding one of
//! the kind's slots in the queue for heavy work (src/slots.rs), waiting in
//! line for one, and exits with the command's code. New with the port.
//!
//! The binary is also the `cargo`, `npm` and `pnpm` every pane finds first
//! on its PATH (`STAND_INS`, linked in `bin_dir`): run by one of those
//! names, it finds the real program further down the PATH (a `cargo` the
//! PATH lacks in rustup's usual folders, `platform::rustup_dirs`, since
//! Homebrew's rustup leaves its folder off the PATH) and runs a heavy
//! subcommand of it through a slot of its kind (`heavy`), and anything else
//! at once, in its place. `cargo test` is two steps, so a compile holds a
//! build slot only while it compiles and a long test run never keeps a
//! build out: `cargo test --no-run` with a build slot, then the same `cargo
//! test`, which finds everything built, with a test run slot. A command
//! holding a slot runs with `SWITCHBOARD_SLOT` set, and nothing it starts
//! waits for another; with Settings' "Queue heavy work" off, or a kind's cap
//! at 0, the stand-in runs the command at once, as the real program would.
//!
//! A compile is given `CARGO_BUILD_JOBS` (and a test run `RUST_TEST_THREADS`
//! and nextest's `NEXTEST_TEST_THREADS`) at the cores over the kind's cap,
//! unless the command's environment sets them, so the slots share the
//! cores rather than each taking them all.

use std::ffi::OsString;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use crate::paths;
use crate::slots::{self, Kind};

/// The programs the binary stands in for, by name.
pub const STAND_INS: [&str; 3] = ["cargo", "npm", "pnpm"];

/// Where the stand-ins' links are, first on every pane's PATH:
/// `~/.local/share/switchboard/bin`.
pub fn bin_dir() -> PathBuf {
    paths::home_join(".local/share/switchboard/bin")
}

/// Each of `STAND_INS` in `bin_dir` as a link to this binary, made where it
/// isn't one already. The server makes them as it starts.
pub fn link_stand_ins() -> std::io::Result<()> {
    let exe = crate::util::exe()?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let bin = bin_dir();
    std::fs::create_dir_all(&bin)?;
    for name in STAND_INS {
        let link = bin.join(name);
        if std::fs::read_link(&link).is_ok_and(|to| to == exe) {
            continue;
        }
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&exe, &link)?;
    }
    Ok(())
}

/// A PATH with `bin_dir` first, and nowhere else.
pub fn path_with_stand_ins(path: &str) -> String {
    let bin = bin_dir();
    let bin = bin.to_string_lossy();
    let rest: Vec<&str> = path
        .split(':')
        .filter(|p| !p.is_empty() && *p != bin)
        .collect();
    std::iter::once(bin.as_ref())
        .chain(rest)
        .collect::<Vec<_>>()
        .join(":")
}

/// How a stand-in runs a command.
#[derive(Debug, PartialEq)]
pub enum Plan {
    /// At once, holding nothing.
    AtOnce,
    /// Holding a slot of one kind.
    Hold(Kind),
    /// `cargo test`: these arguments (the compile alone) with a build
    /// slot, then the command's own with a test run slot.
    CompileThenTest(Vec<String>),
}

/// The flags cargo takes before its subcommand that take a value of their
/// own as the next word.
const CARGO_VALUED: [&str; 5] = ["--config", "-Z", "--color", "-C", "--explain"];

/// cargo's subcommand: the first word that isn't a flag (or a flag's
/// value) or `+toolchain`, with its place.
fn subcommand(args: &[String], valued: &[&str]) -> Option<(usize, String)> {
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if valued.contains(&a.as_str()) {
            i += 2;
            continue;
        }
        if a.starts_with('-') || a.starts_with('+') {
            i += 1;
            continue;
        }
        return Some((i, a.clone()));
    }
    None
}

/// How `program` run with `args` goes through the queue.
pub fn heavy(program: &str, args: &[String]) -> Plan {
    match program {
        "cargo" => {
            let Some((at, sub)) = subcommand(args, &CARGO_VALUED) else {
                return Plan::AtOnce;
            };
            // Asking for help or the version builds nothing.
            let own = &args[..args.iter().position(|a| a == "--").unwrap_or(args.len())];
            if own
                .iter()
                .any(|a| matches!(a.as_str(), "-h" | "--help" | "-V" | "--version"))
            {
                return Plan::AtOnce;
            }
            match sub.as_str() {
                "build" | "b" | "check" | "c" | "clippy" | "doc" | "d" | "rustc" | "rustdoc"
                | "fix" | "install" => Plan::Hold(Kind::Build),
                "test" | "t" | "bench" => {
                    let own = &args[..args.iter().position(|a| a == "--").unwrap_or(args.len())];
                    if own.iter().any(|a| a == "--no-run") {
                        return Plan::Hold(Kind::Build);
                    }
                    // Doc tests compile as they run, and cargo refuses
                    // --no-run with --doc: one test slot throughout.
                    if own.iter().any(|a| a == "--doc") {
                        return Plan::Hold(Kind::Tests);
                    }
                    let mut compile: Vec<String> = own.to_vec();
                    compile.insert(at + 1, "--no-run".to_string());
                    Plan::CompileThenTest(compile)
                }
                // nextest's own compile can't be had apart from its run, so
                // its run holds a test run slot throughout, its compile
                // sharing the cores as the builds do.
                // Its command past its own options (`nextest --color never
                // run`): the first of its commands among the words after it.
                "nextest" => {
                    let rest = &args[at + 1..];
                    let own = &rest[..rest.iter().position(|a| a == "--").unwrap_or(rest.len())];
                    match own
                        .iter()
                        .map(String::as_str)
                        .find(|w| matches!(*w, "run" | "r" | "archive" | "list"))
                    {
                        Some("run" | "r") => Plan::Hold(Kind::Tests),
                        Some(_) => Plan::Hold(Kind::Build),
                        None => Plan::AtOnce,
                    }
                }
                _ => Plan::AtOnce,
            }
        }
        "npm" | "pnpm" => {
            let asks = |a: &String| matches!(a.as_str(), "-v" | "--version" | "-h" | "--help");
            // Heavy when any of the command's own words (those before a
            // `--` and its script's arguments, flags aside) names an install
            // or a build: the options npm and pnpm take, with or without a
            // value, can't all be known, so a value that looks like one of
            // these only makes a light command wait its turn, and none hides
            // a build from the queue.
            let own = &args[..args.iter().position(|a| a == "--").unwrap_or(args.len())];
            let words: Vec<&str> = own
                .iter()
                .map(String::as_str)
                .filter(|a| !a.starts_with('-'))
                .collect();
            let installs = words.iter().any(|w| {
                matches!(
                    *w,
                    "ci" | "install"
                        | "i"
                        | "add"
                        | "rebuild"
                        | "clean-install"
                        | "install-clean"
                        | "ic"
                        | "cit"
                        | "install-test"
                        | "it"
                        | "install-ci-test"
                        | "isntall"
                )
            });
            // pnpm with no command, only its options and their values
            // (`pnpm --dir web --frozen-lockfile`), installs: every word
            // follows an option, so it is taken for that option's value.
            let lone = own.iter().enumerate().any(|(i, a)| {
                !a.starts_with('-')
                    && (i == 0 || !own[i - 1].starts_with('-') || own[i - 1].contains('='))
            });
            let pnpm_installs = program == "pnpm" && !lone && !own.iter().any(asks);
            let run_at = words
                .iter()
                .position(|w| matches!(*w, "run" | "run-script"));
            let runs_build = run_at.is_some_and(|at| words[at + 1..].contains(&"build"));
            let pnpm_build = program == "pnpm" && words.contains(&"build");
            if installs || runs_build || pnpm_build || pnpm_installs {
                Plan::Hold(Kind::Js)
            } else {
                Plan::AtOnce
            }
        }
        _ => Plan::AtOnce,
    }
}

/// The real `program`: the first on `path` that isn't this binary (or a
/// link to it) and isn't in `bin_dir`, else, for `cargo`, the first in
/// `rustup_dirs` (`platform::rustup_dirs`), since Homebrew's rustup leaves
/// its folder off the PATH.
pub fn real_program(
    program: &str,
    path: &str,
    me: Option<&Path>,
    rustup_dirs: &[PathBuf],
) -> Option<PathBuf> {
    let bin = bin_dir();
    let on_path = path
        .split(':')
        .filter(|dir| !dir.is_empty() && Path::new(dir) != bin)
        .map(PathBuf::from);
    let rustups = rustup_dirs.iter().filter(|_| program == "cargo").cloned();
    on_path
        .chain(rustups)
        .map(|dir| dir.join(program))
        .find(|candidate| {
            let Ok(meta) = std::fs::metadata(candidate) else {
                return false;
            };
            let runnable = meta.is_file()
                && std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o111 != 0;
            runnable
                && me.is_none_or(|me| std::fs::canonicalize(candidate).ok().as_deref() != Some(me))
        })
}

/// `path` with `real`'s folder added at its end when it isn't on it, so a
/// cargo found in rustup's folders finds `rustc` beside it; `None` when
/// nothing needs adding.
pub(crate) fn path_reaching(real: &Path, path: &str) -> Option<String> {
    let dir = real.parent()?;
    if path.split(':').any(|d| Path::new(d) == dir) {
        return None;
    }
    let dir = dir.to_string_lossy();
    Some(if path.is_empty() {
        dir.into_owned()
    } else {
        format!("{path}:{dir}")
    })
}

/// What a command given one of `cap` slots of `kind` runs with, besides
/// `SWITCHBOARD_SLOT`: the cores shared out, where the environment leaves
/// it to us.
fn shares(kind: Kind, cap: usize) -> Vec<(&'static str, String)> {
    let jobs = slots::share(cap, slots::cores()).to_string();
    // A test run that compiles too (nextest, doc tests) compiles with a
    // build's share of the cores.
    let build_jobs = slots::share(slots::cap(Kind::Build), slots::cores()).to_string();
    let names: Vec<(&'static str, String)> = match kind {
        Kind::Build => vec![("CARGO_BUILD_JOBS", jobs)],
        Kind::Tests => vec![
            ("RUST_TEST_THREADS", jobs.clone()),
            ("NEXTEST_TEST_THREADS", jobs),
            ("CARGO_BUILD_JOBS", build_jobs),
        ],
        Kind::Js => Vec::new(),
    };
    names
        .into_iter()
        .filter(|(n, _)| std::env::var_os(n).is_none())
        .collect()
}

/// A slot held, of which kind and cap, and the environment its command
/// runs with.
struct Taken {
    held: slots::Held,
    env: Vec<(&'static str, String)>,
    kind: Kind,
    cap: usize,
}

/// Takes a slot of `kind` (saying so once on stderr while waiting), when
/// the queue gives the kind any: the slot, held until dropped, and the
/// environment to run with. `None` with the queue off for the kind.
fn take(kind: Kind, words: &str) -> Result<Option<Taken>, String> {
    let cap = slots::cap(kind);
    if cap == 0 {
        return Ok(None);
    }
    let pid = std::process::id().to_string();
    // Settings are read again at each look, so a cap changed (or the
    // queue turned off) while it waits applies to it.
    let held = slots::take(
        kind,
        || slots::cap_now(kind),
        |place| {
            eprintln!(
                "Waiting for a {} ({} in line)",
                kind.words(),
                slots::ordinal(place)
            );
            tell(&["wait", kind.name(), &pid, "--", words]);
        },
    )
    .map_err(|e| format!("{e:#}"))?;
    let Some(held) = held else {
        // The queue was turned off for it while it waited.
        tell(&["done", &pid]);
        return Ok(None);
    };
    let cap = held.cap;
    let mut env = shares(kind, cap);
    env.push((slots::HELD, kind.name().to_string()));
    Ok(Some(Taken {
        held,
        env,
        kind,
        cap,
    }))
}

/// Tells the server the slot is held, and by which command (`child`), so
/// its tab shows it and the Resources view can stop it.
fn tell_held(taken: &Taken, child: u32, words: &str) {
    tell(&[
        "hold",
        taken.kind.name(),
        &std::process::id().to_string(),
        &taken.held.index.to_string(),
        &taken.cap.to_string(),
        &child.to_string(),
        "--",
        words,
    ]);
}

/// Tells the workspace's server, when this runs in one of its panes, where
/// the command stands with its slot (`slot-report`), so its tab shows it:
/// waiting at most a second, and never failing the command.
fn tell(words: &[&str]) {
    if !crate::mux::socket::in_pane() {
        return;
    }
    let mut args = vec!["slot-report".to_string()];
    args.extend(words.iter().map(|w| w.to_string()));
    let _ = crate::mux::socket::send_quiet(args, std::env::var("SWITCHBOARD_PANE").ok(), None);
}

/// The command in words, as its tab shows it, cut at 200 characters.
fn words_of(argv0: &str, args: &[String]) -> String {
    let all = std::iter::once(argv0)
        .chain(args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ");
    all.chars().take(200).collect()
}

fn code_of(status: std::process::ExitStatus) -> u8 {
    match (status.code(), status.signal()) {
        (Some(code), _) => code as u8,
        (None, Some(signal)) => 128u8.saturating_add(signal as u8),
        (None, None) => 1,
    }
}

/// Runs `program` with `args` holding a slot of `kind` (or at once, with
/// the queue off for it): its exit code.
fn run_held(
    program: &Path,
    args: &[String],
    kind: Kind,
    argv0: &str,
    path: Option<&str>,
) -> Result<u8, String> {
    let words = words_of(argv0, args);
    let taken = take(kind, &words)?;
    let mut cmd = Command::new(program);
    cmd.arg0(argv0).args(args);
    if let Some(path) = path {
        cmd.env("PATH", path);
    }
    if let Some(t) = &taken {
        cmd.envs(t.env.iter().map(|(k, v)| (*k, v)));
    }
    let status = cmd.spawn().and_then(|mut child| {
        if let Some(t) = &taken {
            tell_held(t, child.id(), &words);
        }
        child.wait()
    });
    let status = status.map_err(|e| format!("running {}: {e}", program.display()));
    if taken.is_some() {
        drop(taken);
        tell(&["done", &std::process::id().to_string()]);
    }
    Ok(code_of(status?))
}

/// Replaces this process with `program`, as though it had been run.
fn exec(program: &Path, args: &[String], argv0: &str, path: Option<&str>) -> ExitCode {
    let mut cmd = Command::new(program);
    cmd.arg0(argv0).args(args);
    if let Some(path) = path {
        cmd.env("PATH", path);
    }
    let err = cmd.exec();
    eprintln!("{argv0}: running {}: {err}", program.display());
    ExitCode::from(126)
}

/// The binary run as `cargo`, `npm` or `pnpm`.
pub fn stand_in(name: &str, args: Vec<String>) -> ExitCode {
    let path = std::env::var("PATH").unwrap_or_default();
    let me = crate::util::exe()
        .ok()
        .and_then(|e| std::fs::canonicalize(e).ok());
    let Some(real) = real_program(name, &path, me.as_deref(), &crate::platform::rustup_dirs())
    else {
        if name == "cargo" {
            eprintln!("{name}: {name} isn't on your PATH or in rustup's usual folders");
        } else {
            eprintln!("{name}: command not found");
        }
        return ExitCode::from(127);
    };
    let child_path = path_reaching(&real, &path);
    let child_path = child_path.as_deref();
    if std::env::var_os(slots::HELD).is_some() {
        return exec(&real, &args, name, child_path);
    }
    // With the queue off for the work, the command runs as typed.
    let result = match heavy(name, &args) {
        Plan::AtOnce => return exec(&real, &args, name, child_path),
        Plan::Hold(kind) if slots::cap(kind) == 0 => {
            return exec(&real, &args, name, child_path);
        }
        Plan::CompileThenTest(_)
            if slots::cap(Kind::Build) == 0 && slots::cap(Kind::Tests) == 0 =>
        {
            return exec(&real, &args, name, child_path);
        }
        Plan::Hold(kind) => run_held(&real, &args, kind, name, child_path),
        Plan::CompileThenTest(compile) => {
            match run_held(&real, &compile, Kind::Build, name, child_path) {
                Ok(0) => run_held(&real, &args, Kind::Tests, name, child_path),
                other => other,
            }
        }
    };
    match result {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("{name}: the queue for heavy work: {e}");
            ExitCode::from(1)
        }
    }
}

/// `switchboard slot <kind> -- <command> [args]`.
pub fn main(args: Vec<String>) -> ExitCode {
    let usage = || {
        eprintln!(
            "usage: switchboard slot <build|tests|js> -- <command> [args]\n\
             Runs the command holding one of the kind's slots in the queue for heavy work, waiting in line for one."
        );
        ExitCode::from(2)
    };
    let mut args = args.into_iter();
    let Some(kind) = args.next().as_deref().and_then(Kind::from_name) else {
        return usage();
    };
    let rest: Vec<String> = args.collect();
    let rest = match rest.first().map(String::as_str) {
        Some("--") => rest[1..].to_vec(),
        _ => rest,
    };
    let Some((program, program_args)) = rest.split_first() else {
        return usage();
    };
    let program = PathBuf::from(OsString::from(program));
    let shown = program.to_string_lossy().into_owned();
    let result = if std::env::var_os(slots::HELD).is_some() {
        Command::new(&program)
            .args(program_args)
            .status()
            .map(code_of)
            .map_err(|e| format!("running {shown}: {e}"))
    } else {
        run_held(&program, program_args, kind, &shown, None)
    };
    match result {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("switchboard slot: {e}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn heavy_cargo_work_takes_its_kind_and_the_rest_runs_at_once() {
        for (line, plan) in [
            ("build --release --locked", Plan::Hold(Kind::Build)),
            ("+nightly -q check", Plan::Hold(Kind::Build)),
            ("--config a=b clippy --all-targets", Plan::Hold(Kind::Build)),
            ("test --no-run", Plan::Hold(Kind::Build)),
            ("nextest run --test-threads 8", Plan::Hold(Kind::Tests)),
            ("nextest --color never run", Plan::Hold(Kind::Tests)),
            ("test --doc", Plan::Hold(Kind::Tests)),
            ("fmt", Plan::AtOnce),
            ("metadata --format-version 1", Plan::AtOnce),
            ("--version", Plan::AtOnce),
            ("test --help", Plan::AtOnce),
            ("build -h", Plan::AtOnce),
            (
                "test -- --help",
                Plan::CompileThenTest(words("test --no-run")),
            ),
            ("run -- serve", Plan::AtOnce),
        ] {
            assert_eq!(heavy("cargo", &words(line)), plan, "cargo {line}");
        }
    }

    #[test]
    fn cargo_test_compiles_first_with_no_run_before_the_test_binarys_arguments() {
        assert_eq!(
            heavy("cargo", &words("-q test --test it setup:: -- --nocapture")),
            Plan::CompileThenTest(words("-q test --no-run --test it setup::"))
        );
        assert_eq!(
            heavy("cargo", &words("t")),
            Plan::CompileThenTest(words("t --no-run"))
        );
    }

    #[test]
    fn package_installs_and_builds_take_a_package_slot() {
        for (program, line, plan) in [
            ("npm", "ci", Plan::Hold(Kind::Js)),
            ("npm", "install", Plan::Hold(Kind::Js)),
            ("npm", "run build", Plan::Hold(Kind::Js)),
            ("npm", "run test", Plan::AtOnce),
            ("npm", "view react version", Plan::AtOnce),
            ("pnpm", "", Plan::Hold(Kind::Js)),
            ("pnpm", "install --frozen-lockfile", Plan::Hold(Kind::Js)),
            ("pnpm", "build", Plan::Hold(Kind::Js)),
            ("pnpm", "--version", Plan::AtOnce),
            ("pnpm", "--frozen-lockfile", Plan::Hold(Kind::Js)),
            ("pnpm", "--dir web --frozen-lockfile", Plan::Hold(Kind::Js)),
            ("pnpm", "lint", Plan::AtOnce),
            ("npm", "clean-install", Plan::Hold(Kind::Js)),
            ("npm", "--help", Plan::AtOnce),
            ("pnpm", "--filter web build", Plan::Hold(Kind::Js)),
            ("npm", "run --workspace web build", Plan::Hold(Kind::Js)),
            (
                "pnpm",
                "run --reporter append-only build",
                Plan::Hold(Kind::Js),
            ),
            ("pnpm", "--store-dir /a/store install", Plan::Hold(Kind::Js)),
            (
                "pnpm",
                "--workspace-concurrency 1 --recursive run build",
                Plan::Hold(Kind::Js),
            ),
            ("pnpm", "--silent view react", Plan::AtOnce),
            ("pnpm", "--filter-prod test run build", Plan::Hold(Kind::Js)),
            (
                "npm",
                "run --script-shell /bin/sh build",
                Plan::Hold(Kind::Js),
            ),
            (
                "pnpm",
                "-r run --workspace-concurrency 1 build",
                Plan::Hold(Kind::Js),
            ),
            ("npm", "run test -- build", Plan::AtOnce),
            ("npm", "--prefix web install", Plan::Hold(Kind::Js)),
            ("npm", "-w web run build", Plan::Hold(Kind::Js)),
        ] {
            assert_eq!(heavy(program, &words(line)), plan, "{program} {line}");
        }
    }

    #[test]
    fn the_real_program_is_the_first_on_the_path_that_isnt_this_binary() {
        let dir = std::env::temp_dir().join(format!("sb-slot-real-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (mine, other) = (dir.join("mine"), dir.join("other"));
        std::fs::create_dir_all(&mine).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let me = dir.join("switchboard");
        std::fs::write(&me, "").unwrap();
        let script = |p: &Path| {
            std::fs::write(p, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(p, std::os::unix::fs::PermissionsExt::from_mode(0o755))
                .unwrap();
        };
        script(&me);
        std::os::unix::fs::symlink(&me, mine.join("cargo")).unwrap();
        script(&other.join("cargo"));
        let me = std::fs::canonicalize(&me).unwrap();
        let path = format!("{}:{}", mine.display(), other.display());
        assert_eq!(
            real_program("cargo", &path, Some(&me), &[]),
            Some(other.join("cargo"))
        );
        assert_eq!(real_program("npm", &path, Some(&me), &[]), None);
    }

    #[test]
    fn cargo_off_the_path_is_found_in_rustups_folders_and_its_folder_reached() {
        let dir = std::env::temp_dir().join(format!("sb-slot-rustup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (path_dir, cargo_home, brew) = (
            dir.join("path"),
            dir.join("cargo-home/bin"),
            dir.join("opt/rustup/bin"),
        );
        for d in [&path_dir, &cargo_home, &brew] {
            std::fs::create_dir_all(d).unwrap();
        }
        let script = |p: &Path| {
            std::fs::write(p, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(p, std::os::unix::fs::PermissionsExt::from_mode(0o755))
                .unwrap();
        };
        let me = dir.join("switchboard");
        script(&me);
        let me = std::fs::canonicalize(&me).unwrap();
        // The wrapper never finds itself, on the PATH or in rustup's folders.
        std::os::unix::fs::symlink(&me, path_dir.join("cargo")).unwrap();
        std::os::unix::fs::symlink(&me, cargo_home.join("cargo")).unwrap();
        let path = path_dir.to_string_lossy().into_owned();
        let rustups = [cargo_home.clone(), brew.clone()];
        assert_eq!(real_program("cargo", &path, Some(&me), &rustups), None);

        script(&brew.join("cargo"));
        let real = real_program("cargo", &path, Some(&me), &rustups);
        assert_eq!(real, Some(brew.join("cargo")));
        assert_eq!(
            path_reaching(&brew.join("cargo"), &path),
            Some(format!("{path}:{}", brew.display()))
        );
        let out = Command::new(real.unwrap()).output().unwrap();
        assert!(out.status.success());

        // npm is never looked for there.
        script(&brew.join("npm"));
        assert_eq!(real_program("npm", &path, Some(&me), &rustups), None);
        // One on the PATH needs nothing added.
        assert_eq!(path_reaching(&path_dir.join("cargo"), &path), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_stand_ins_come_first_on_the_path_once() {
        let bin = bin_dir().to_string_lossy().into_owned();
        assert_eq!(
            path_with_stand_ins(&format!("/a:{bin}:/b")),
            format!("{bin}:/a:/b")
        );
    }
}
