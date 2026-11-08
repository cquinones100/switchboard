//! `leak-check <range>`: anything that identifies you or your employer in
//! what a range of commits adds, before it is pushed to a public repository.
//! Ported from bin/leak-check.
//!
//! It reads each line any of the range's commits adds (`git log -p -m
//! <range>`, every commit and a merge against each parent, so a line a later
//! commit removes still counts, since the push carries every commit; files
//! the repository marks binary, and its textconv drivers, left out of it, so
//! they are read as they are) and each commit message, and finds:
//!
//! - the words in `~/.config/switchboard/leak-check.txt`, one a line (your
//!   organization's and company's names, usernames, internal hosts; blank
//!   lines and lines starting with # left out), as whole words, any case;
//! - Linear-style ticket IDs (a team key, a dash, a number) and PR numbers
//!   written as a # and three digits or more, except in tests, whose IDs are
//!   made up (and ABC-…, XYZ-…, #123 and #12345 anywhere, the made-up ones
//!   the docs use);
//! - home directory paths (/Users/<name>), email addresses (but noreply and
//!   example ones), and token shapes (GitHub's, OpenAI-style sk-, Slack's).
//!
//! It prints each hit as `<file>:<line>: <kind>: <text>`, or `commit
//! <hash>: <kind>: <text>` for a message, and exits 1 on any, 0 on none,
//! and 2 when it can't look: a word list that is there but can't be read
//! stops it, so a landing asks rather than pushing unchecked.
//! queue-and-clean runs it before a direct landing asks to push, and a
//! landing without asking (autopilot's "Land without asking")
//! asks after all when it finds anything.
//!
//! JavaScript's look-arounds, which Rust's regex hasn't, are the boundary
//! checks of `bounded`, tried again one character on wherever one fails,
//! as a backtracking search would.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::LazyLock;

use regex::Regex;

/// The file of words to find, in your own files.
pub fn words_file() -> PathBuf {
    crate::paths::config_dir().join("leak-check.txt")
}

/// The words of the list, or why it can't be read; none when there is no
/// list.
pub fn read_words(file: &Path) -> Result<Vec<String>, String> {
    match std::fs::read_to_string(file) {
        Ok(text) => Ok(text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(str::to_string)
            .collect()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("can't read {}: {e}", file.display())),
    }
}

/// Standards, and key names (M-1, C-2), that look like ticket IDs.
const NOT_TICKETS: &[&str] = &[
    "UTF", "SHA", "ISO", "RFC", "CVE", "HTTP", "TLS", "SSL", "AES", "RSA", "ECMA", "IPV", "UTC",
    "GMT", "ASCII", "ABC", "XYZ", "CP", "MD", "ES", "WCAG", "BASE", "LF", "CRLF", "PEP", "POSIX",
    "IEEE", "ANSI", "OSC", "CSI", "DCS", "SGR", "DEC", "VT", "XTERM", "TODO", "NOTE", "UID", "GID",
    "PID", "SIG", "MPL", "GPL", "LGPL", "AGPL", "BSD", "CC", "APACHE", "M", "C", "S",
];
const PLACEHOLDER_USERS: &[&str] = &[
    "me", "you", "someone", "name", "user", "username", "example", "test", "x", "shared",
];

static TICKET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([A-Z][A-Z0-9]*)-[0-9]+").unwrap());
static PR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"#[0-9]{3,}").unwrap());
static COLOR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^#([0-9]{3}|[0-9]{6})$").unwrap());
// A color (color: #202124, "background": "#000000", bg = "#000") is no PR:
// one of 3 or 6 digits given as a color property's value.
static COLOR_BEFORE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)(colou?r|background|fill|stroke|border|\b[bf]g)[A-Za-z0-9_-]*["']?\s*[:=]\s*["']?$"#,
    )
    .unwrap()
});
static HOME_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/Users/([A-Za-z][A-Za-z0-9_.-]*)").unwrap());
static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[A-Za-z0-9_.+-]+@[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}").unwrap()
});
// Made-up and generic addresses: noreply and example ones, git's SSH user
// and a placeholder user in a URL at the big git hosts, and in tests any at
// the big hosts, whose calendar event IDs look like addresses.
static OK_EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(^|[.@])(noreply|no-reply)[@.]|@(example\.(com|org|net)|[A-Za-z0-9_.-]+\.example|localhost)$|@users\.noreply\.github\.com$|^(git|me|you|user|someone)@((ssh\.)?github\.com|gitlab\.com|bitbucket\.org)$",
    )
    .unwrap()
});
static OK_TEST_EMAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)@(google|github|gitlab)\.com$").unwrap());
static TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|sk-[A-Za-z0-9_-]{20,}|xox[abprs]-[A-Za-z0-9-]{10,}|xapp-[A-Za-z0-9-]{10,})",
    )
    .unwrap()
});
static TEST_DIR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(^|/)(test|tests|__tests__|spec)/").unwrap());
static TEST_FILE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\.(test|spec)\.[^/]+$").unwrap());
static HUNK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^@@ -\S+ \+([0-9]+)").unwrap());
static CLAUDE_TRAILER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^Co-Authored-By: Claude\b").unwrap());

/// A word character, as JavaScript's `\w` reads it.
fn word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The character before and after a match.
fn around(text: &str, start: usize, end: usize) -> (Option<char>, Option<char>) {
    (
        text[..start].chars().next_back(),
        text[end..].chars().next(),
    )
}

/// Each match of `re` in `text` whose surroundings `ok` takes, as a search
/// with look-arounds finds them: where they fail at a place, the search goes
/// on from the next character rather than past the match.
fn bounded<'t>(
    re: &Regex,
    text: &'t str,
    ok: impl Fn(Option<char>, Option<char>) -> bool,
) -> Vec<regex::Captures<'t>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at <= text.len() {
        let Some(m) = re.captures_at(text, at) else {
            break;
        };
        let whole = m.get(0).expect("a match");
        let (before, after) = around(text, whole.start(), whole.end());
        if ok(before, after) {
            at = whole.end().max(whole.start() + 1);
            out.push(m);
        } else {
            at = whole.start()
                + text[whole.start()..]
                    .chars()
                    .next()
                    .map_or(1, char::len_utf8);
        }
        while at < text.len() && !text.is_char_boundary(at) {
            at += 1;
        }
    }
    out
}

/// Whether a file is a test's, whose IDs are made up.
pub fn is_test(file: &str) -> bool {
    TEST_DIR.is_match(file) || TEST_FILE.is_match(file)
}

/// What one line holds, `test` whether it is in a test's file: each hit's
/// kind and text.
pub fn hits_in(text: &str, test: bool, words: &[String]) -> Vec<(&'static str, String)> {
    let mut hits = Vec::new();
    for w in words {
        let Ok(re) = Regex::new(&format!("(?i){}", regex::escape(w))) else {
            continue;
        };
        let found = bounded(&re, text, |b, a| {
            !b.is_some_and(word) && !a.is_some_and(word)
        });
        if let Some(m) = found.first() {
            hits.push(("listed word", m[0].to_string()));
        }
    }
    if !test {
        let ticket = |b: Option<char>, a: Option<char>| {
            !b.is_some_and(|c| word(c) || c == '-') && !a.is_some_and(word)
        };
        for m in bounded(&TICKET, text, ticket) {
            if !NOT_TICKETS.contains(&&m[1]) {
                hits.push(("ticket ID", m[0].to_string()));
            }
        }
        let pr = |b: Option<char>, a: Option<char>| {
            !b.is_some_and(|c| word(c) || matches!(c, '&' | '#' | '/')) && !a.is_some_and(word)
        };
        for m in bounded(&PR, text, pr) {
            let whole = m.get(0).expect("a match");
            let number = whole.as_str();
            let color = COLOR.is_match(number) && COLOR_BEFORE.is_match(&text[..whole.start()]);
            if !["#123", "#12345"].contains(&number) && !color {
                hits.push(("PR number", number.to_string()));
            }
        }
    }
    for m in HOME_PATH.captures_iter(text) {
        if !PLACEHOLDER_USERS.contains(&m[1].to_lowercase().as_str()) {
            hits.push(("home path", m[0].to_string()));
        }
    }
    let email = |b: Option<char>, a: Option<char>| {
        !b.is_some_and(|c| word(c) || matches!(c, '.' | '+' | '-'))
            && !a.is_some_and(|c| word(c) || c == '-')
    };
    for m in bounded(&EMAIL, text, email) {
        let address = &m[0];
        if !OK_EMAIL.is_match(address) && !(test && OK_TEST_EMAIL.is_match(address)) {
            hits.push(("email address", address.to_string()));
        }
    }
    let token = |b: Option<char>, _: Option<char>| !b.is_some_and(|c| word(c) || c == '-');
    for m in bounded(&TOKEN, text, token) {
        let shown: String = m[0].chars().take(8).collect();
        hits.push(("token", format!("{shown}…")));
    }
    hits
}

/// Every hit in `git log -p -m`'s added lines and in the commit messages
/// `--format=%h%x00%B%x00` printed, each once, in the order found.
pub fn hits(diff: &str, log: &str, words: &[String]) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut file = String::new();
    let mut line = 0usize;
    for row in diff.split('\n') {
        if row.starts_with("diff --git ") {
            file.clear();
            continue;
        }
        if let Some(name) = row.strip_prefix("+++ ") {
            file = if name == "/dev/null" {
                String::new()
            } else {
                name.strip_prefix("b/").unwrap_or(name).to_string()
            };
            continue;
        }
        if let Some(m) = HUNK.captures(row) {
            line = m[1].parse().unwrap_or(0);
            continue;
        }
        let Some(added) = row.strip_prefix('+') else {
            continue;
        };
        if file.is_empty() {
            continue;
        }
        for (kind, text) in hits_in(added, is_test(&file), words) {
            found.push(format!("{file}:{line}: {kind}: {text}"));
        }
        line += 1;
    }
    let parts: Vec<&str> = log.split('\0').collect();
    for pair in parts.chunks(2) {
        let [hash, message] = pair else {
            break;
        };
        let hash = hash.trim();
        // The trailers Claude Code adds name a model, not you.
        let message: Vec<&str> = message
            .split('\n')
            .filter(|l| !CLAUDE_TRAILER.is_match(l))
            .collect();
        for (kind, text) in hits_in(&message.join("\n"), false, words) {
            found.push(format!("commit {hash}: {kind}: {text}"));
        }
    }
    let mut seen = HashSet::new();
    found.retain(|hit| seen.insert(hit.clone()));
    found
}

fn git(args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn main(args: Vec<String>) -> ExitCode {
    let [range] = args.as_slice() else {
        eprintln!("usage: leak-check <range>");
        return ExitCode::from(2);
    };
    let words = match read_words(&words_file()) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("leak-check: {e}");
            return ExitCode::from(2);
        }
    };
    let looked = git(&[
        "log",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--text",
        "--unified=0",
        "--no-renames",
        "-p",
        "-m",
        "--format=",
        range,
        "--",
    ])
    .and_then(|diff| {
        let log = git(&[
            "log",
            "--no-color",
            "--no-textconv",
            "--format=%h%x00%B%x00",
            range,
            "--",
        ])?;
        Ok((diff, log))
    });
    let (diff, log) = match looked {
        Ok(both) => both,
        Err(e) => {
            eprintln!("leak-check: {e}");
            return ExitCode::from(2);
        }
    };
    let found = hits(&diff, &log, &words);
    for hit in &found {
        println!("{hit}");
    }
    ExitCode::from(if found.is_empty() { 0 } else { 1 })
}
