//! The rules a sealed try runs under (`switchboard try --sealed`), written
//! in the language of macOS's sandbox profiles (SBPL), which `sandbox-exec`
//! applies to a program and everything it starts. Ported from
//! bin/switchboard-try's profile. Only macOS has the sandbox
//! (`platform::sandboxed`); the text is built here on any platform, so its
//! rules are tested wherever the tests run.
//!
//! A profile's rules are read in order, the last that matches winning, and
//! name real paths (`/private/tmp`, not `/tmp`), as the kernel checks them.
//! Inside it a program can't:
//!
//! - read your home or your per-user temporary and cache folders, but the
//!   folders it is given to read (the worktree, its git directory, the
//!   toolchain) and the try's home, nor see `~/.cargo` at all;
//! - write anywhere but the try's home and `/tmp`, and there not in the
//!   worktree, its git directory, Claude's or tmux's folders of yours,
//!   another try's home or a try's home being made, nor leave in `/tmp`
//!   itself what a program of yours run anywhere under it would find there
//!   and take up (a `.git`, `.cargo`, `CLAUDE.md`, `package.json`);
//! - reach the network, listen on it, or connect to any socket but those in
//!   the try's home (your Switchboard's, your ssh-agent's, every Neovim's
//!   and the Notes app's among them, which take commands from whoever
//!   connects); for `--run`, the tests', localhost and a test's own sockets
//!   in a folder under `/tmp` named as the tests name theirs;
//! - signal a process outside the sandbox;
//! - ask the Keychain, the clipboard, Launch Services (so no `open`) or the
//!   window server, or send an Apple Event, through which another app would
//!   run a command for it.
//!
//! `client` is the narrower profile a program of Switchboard's own runs in
//! to reach a sealed try's server (a session made, the server stopped):
//! whatever answers on that socket is the try's code, so it can read
//! nothing of yours, run nothing, write nothing and reach no other socket.

use std::path::{Path, PathBuf};

/// The mach services nothing in a sealed try may reach: the Keychain's
/// (two), the clipboard's, Launch Services' and the window server's, which
/// can post keys to other apps.
const SERVICES: &[&str] = &[
    "com.apple.SecurityServer",
    "com.apple.securityd.xpc",
    "com.apple.pasteboard.1",
    "com.apple.coreservices.launchservicesd",
    "com.apple.windowserver.active",
];

/// Where a sealed try runs, every path a real one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rules {
    /// The try's home, which it may read and write.
    pub home: PathBuf,
    /// Your home, which it may neither read nor write.
    pub real_home: PathBuf,
    /// Your per-user temporary and cache folders, when the computer has them.
    pub user_dir: Option<PathBuf>,
    /// `/tmp`, where the tries' homes are: `/private/tmp` on a Mac.
    pub tmp: PathBuf,
    /// Your user id, which names your own folders in `/tmp`.
    pub uid: u32,
    /// What it may read though it is in your home: the worktree, its git
    /// directory, the toolchain.
    pub readable: Vec<PathBuf>,
    /// What it may never write though `/tmp` would let it: the worktree and
    /// its git directory.
    pub unwritable: Vec<PathBuf>,
    /// For `--run`: localhost, and a test's own sockets.
    pub run: bool,
}

/// A path as an SBPL string.
fn quoted(path: &Path) -> String {
    let text = path.to_string_lossy();
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn subpath(path: &Path) -> String {
    format!("(subpath {})", quoted(path))
}

/// A path as the start of an SBPL regex, its special characters escaped.
fn regex_of(path: &Path) -> String {
    let mut out = String::new();
    for c in path.to_string_lossy().chars() {
        if "\\^$.|?*+()[]{}\"".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Every try's home, and every one being made (`.new`): `sb-` and eight
/// hex digits under `/tmp`, written out, since SBPL's regexes have no
/// `{8}`.
fn tries(tmp: &Path) -> String {
    format!(
        "(regex #\"^{}/sb-{}\")",
        regex_of(tmp),
        "[0-9a-f]".repeat(8)
    )
}

/// What a program looks for in every folder above where it runs, and so
/// would take from `/tmp` itself when run anywhere under it: git's
/// repository, cargo's and rustup's settings, npm's package, what Claude
/// Code and Codex load, direnv's file. None of it may be left there for a
/// program of yours to find.
fn found_above(tmp: &Path) -> String {
    format!(
        "(regex #\"^{}/(\\.|(CLAUDE|CLAUDE\\.local|AGENTS)\\.md$|Cargo\\.(toml|lock)$|rust-toolchain(\\.toml)?$|package\\.json$)\")",
        regex_of(tmp)
    )
}

fn denied_services() -> String {
    let names: Vec<String> = SERVICES
        .iter()
        .map(|s| format!("(global-name \"{s}\")"))
        .collect();
    format!("(deny mach-lookup {})", names.join(" "))
}

/// The profile a sealed try's every process runs under.
pub fn profile(r: &Rules) -> String {
    let tmp = &r.tmp;
    let claude = tmp.join(format!("claude-{}", r.uid));
    let tmux = tmp.join(format!("tmux-{}", r.uid));
    let launchd = format!("(regex #\"^{}/com\\.apple\\.launchd\\.\")", regex_of(tmp));
    let theirs = [subpath(&claude), subpath(&tmux), tries(tmp), launchd];
    let mut no_read: Vec<String> = r.user_dir.iter().map(|d| subpath(d)).collect();
    no_read.push(subpath(&r.real_home));
    no_read.extend(theirs.iter().cloned());
    let mut no_write: Vec<String> = theirs.to_vec();
    no_write.push(found_above(tmp));
    no_write.push(subpath(&r.real_home));
    no_write.extend(r.unwritable.iter().map(|p| subpath(p)));
    let readable: Vec<String> = r.readable.iter().map(|p| subpath(p)).collect();
    let home = subpath(&r.home);
    let mut lines = vec![
        "(version 1)".to_string(),
        "(allow default)".to_string(),
        format!("(deny file-read-data {})", no_read.join(" ")),
        // Not even seen: cargo reads the .cargo/config.toml of every folder
        // above where it runs, your home's among them for a worktree in it,
        // and fails on one it can't read, where one it doesn't find is
        // passed over.
        format!(
            "(deny file-read-metadata {})",
            subpath(&r.real_home.join(".cargo"))
        ),
        "(deny file-write* (subpath \"/\"))".to_string(),
        format!("(allow file-write* {} (subpath \"/dev\"))", subpath(tmp)),
        format!("(deny file-write* {})", no_write.join(" ")),
    ];
    if !readable.is_empty() {
        lines.push(format!("(allow file-read-data {})", readable.join(" ")));
    }
    lines.extend([
        format!("(allow file-read-data file-write* {home})"),
        "(deny network-outbound (remote ip) (remote unix-socket))".to_string(),
        "(deny network-inbound (local ip))".to_string(),
        "(deny network-bind (local ip))".to_string(),
    ]);
    if r.run {
        // The tests' own folders, as tests/common names them: a name, its
        // process and a number, under /tmp.
        lines.push(format!(
            "(allow network-outbound (remote unix-socket (path-regex #\"^{}/[A-Za-z][A-Za-z0-9_-]*-[0-9]+-[0-9]+/\")))",
            regex_of(tmp)
        ));
        lines.push("(allow network-outbound (remote ip \"localhost:*\"))".to_string());
        lines.push("(allow network-bind network-inbound (local ip \"localhost:*\"))".to_string());
    }
    lines.extend([
        format!("(allow network-outbound (remote unix-socket {home}))"),
        "(deny signal)".to_string(),
        "(allow signal (target same-sandbox))".to_string(),
        "(deny appleevent-send)".to_string(),
        denied_services(),
    ]);
    lines.join("\n") + "\n"
}

/// The profile a program of Switchboard's own (`exe`, its real path) runs
/// under to reach a sealed try's server at its socket in `home`: it reads
/// nothing of your home or per-user folders, runs nothing but itself,
/// writes nothing but to the terminal, signals nothing, and connects to no
/// socket but those in the try's home.
pub fn client(home: &Path, real_home: &Path, user_dir: Option<&Path>, exe: &Path) -> String {
    let mut no_read = vec![subpath(real_home)];
    no_read.extend(user_dir.map(subpath));
    let exe = quoted(exe);
    [
        "(version 1)".to_string(),
        "(allow default)".to_string(),
        format!("(deny file-read-data {})", no_read.join(" ")),
        format!("(allow file-read-data (literal {exe}))"),
        "(deny process-exec*)".to_string(),
        format!("(allow process-exec (literal {exe}))"),
        "(deny file-write* (subpath \"/\"))".to_string(),
        "(allow file-write* (subpath \"/dev\"))".to_string(),
        "(deny network*)".to_string(),
        format!(
            "(allow network-outbound (remote unix-socket {}))",
            subpath(home)
        ),
        "(deny signal)".to_string(),
        "(deny appleevent-send)".to_string(),
        denied_services(),
    ]
    .join("\n")
        + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(run: bool) -> Rules {
        Rules {
            home: "/private/tmp/sb-0123abcd".into(),
            real_home: "/Users/me".into(),
            user_dir: Some("/private/var/folders/xy/abc".into()),
            tmp: "/private/tmp".into(),
            uid: 501,
            readable: vec![
                "/Users/me/.cache/switchboard/pr-review/o-r/1".into(),
                "/Users/me/switchboard/.git".into(),
            ],
            unwritable: vec!["/Users/me/.cache/switchboard/pr-review/o-r/1".into()],
            run,
        }
    }

    /// Where a rule naming `needle` is, by line, the last matching winning.
    fn at(profile: &str, needle: &str) -> usize {
        profile
            .lines()
            .position(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("{needle} in\n{profile}"))
    }

    #[test]
    fn a_sealed_try_reads_and_writes_its_home_and_reaches_nothing_of_yours() {
        let p = profile(&rules(false));
        let deny_read = at(&p, "(deny file-read-data");
        assert!(
            p.lines()
                .nth(deny_read)
                .unwrap()
                .contains("(subpath \"/Users/me\")")
        );
        assert!(p.contains("(subpath \"/private/var/folders/xy/abc\")"));
        assert!(p.contains(
            "(regex #\"^/private/tmp/sb-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]\")"
        ));
        assert!(p.contains("(subpath \"/private/tmp/claude-501\")"));
        // Its home and what it reads come after what it doesn't, so they win.
        let allow_read = at(&p, "(allow file-read-data (subpath");
        let allow_home = at(
            &p,
            "(allow file-read-data file-write* (subpath \"/private/tmp/sb-0123abcd\"))",
        );
        assert!(deny_read < allow_read && allow_read < allow_home);
        let deny_write = at(
            &p,
            "(deny file-write* (subpath \"/private/tmp/claude-501\")",
        );
        assert!(
            p.lines()
                .nth(deny_write)
                .unwrap()
                .contains("pr-review/o-r/1"),
            "the worktree read-only"
        );
        assert!(at(&p, "(allow file-write* (subpath \"/private/tmp\")") < deny_write);
        // No network, no socket but its own, no signal out, no Keychain.
        assert!(p.contains("(deny network-outbound (remote ip) (remote unix-socket))"));
        assert!(
            at(&p, "(deny network-outbound")
                < at(
                    &p,
                    "(allow network-outbound (remote unix-socket (subpath \"/private/tmp/sb-0123abcd\")))"
                )
        );
        assert!(!p.contains("localhost"));
        assert!(p.contains("(allow signal (target same-sandbox))"));
        assert!(p.contains("(deny appleevent-send)"));
        for service in SERVICES {
            assert!(p.contains(service), "{service}");
        }
    }

    #[test]
    fn nothing_is_left_in_tmp_for_a_program_of_yours_to_find_and_cargo_never_sees_yours() {
        let p = profile(&rules(false));
        assert!(p.contains("(deny file-read-metadata (subpath \"/Users/me/.cargo\"))"));
        let line = p
            .lines()
            .find(|l| l.starts_with("(deny file-write* (subpath \"/private/tmp/claude-501\")"))
            .unwrap();
        let pattern = line
            .split("(regex #\"")
            .find(|part| part.starts_with("^/private/tmp/(\\."))
            .and_then(|part| part.split("\")").next())
            .unwrap_or_else(|| panic!("the names found above: {line}"));
        let found = regex::Regex::new(pattern).unwrap();
        for path in [
            "/private/tmp/.git",
            "/private/tmp/.git/config",
            "/private/tmp/.cargo/config.toml",
            "/private/tmp/.claude/settings.json",
            "/private/tmp/CLAUDE.md",
            "/private/tmp/AGENTS.md",
            "/private/tmp/Cargo.toml",
            "/private/tmp/rust-toolchain.toml",
            "/private/tmp/package.json",
        ] {
            assert!(found.is_match(path), "{path}");
        }
        for path in [
            "/private/tmp/sb-0123abcd/.git",
            "/private/tmp/wtpr-1-2/CLAUDE.md",
            "/private/tmp/Cargo.toml.bak",
        ] {
            assert!(!found.is_match(path), "{path}");
        }
    }

    #[test]
    fn run_lets_the_tests_reach_localhost_and_their_own_sockets() {
        let p = profile(&rules(true));
        assert!(p.contains("(allow network-outbound (remote ip \"localhost:*\"))"));
        assert!(
            p.contains("(path-regex #\"^/private/tmp/[A-Za-z][A-Za-z0-9_-]*-[0-9]+-[0-9]+/\")")
        );
        let tests =
            regex::Regex::new("^/private/tmp/[A-Za-z][A-Za-z0-9_-]*-[0-9]+-[0-9]+/").unwrap();
        assert!(tests.is_match("/private/tmp/wtpr-123-4/state/sock"));
        assert!(
            !tests.is_match("/private/tmp/sb-0123abcd/sock"),
            "not a try's home"
        );
        assert!(
            !tests.is_match("/private/tmp/tmux-501/default"),
            "not your tmux"
        );
    }

    #[test]
    fn a_path_is_quoted_and_escaped() {
        assert_eq!(quoted(Path::new("/a \"b\"\\c")), "\"/a \\\"b\\\"\\\\c\"");
        assert_eq!(regex_of(Path::new("/private/tmp.x")), "/private/tmp\\.x");
    }

    #[test]
    fn switchboards_own_client_reaches_the_tries_socket_and_nothing_else() {
        let c = client(
            Path::new("/private/tmp/sb-0123abcd"),
            Path::new("/Users/me"),
            None,
            Path::new("/Users/me/.local/bin/switchboard"),
        );
        assert!(c.contains("(deny process-exec*)\n(allow process-exec (literal \"/Users/me/.local/bin/switchboard\"))"));
        assert!(c.contains("(deny network*)\n(allow network-outbound (remote unix-socket (subpath \"/private/tmp/sb-0123abcd\")))"));
        assert!(c.contains("(deny file-write* (subpath \"/\"))"));
    }
}
