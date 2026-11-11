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
//! A hit in a file's added line is left out when the range's base (the
//! merge base of an `a..b` range's two ends) already has its text in its
//! copy of that file, so a line a change moves or
//! rewrites doesn't count as new while one it brings into a file, or a new
//! file, still does; a commit message's hits always count. The push makes
//! public only what the base doesn't already have. A single revision, which
//! `git log` reads with every commit before it, has no base, so nothing is
//! left out.
//!
//! It prints each hit as `<file>:<line>: <kind>: <text>`, or `commit
//! <hash>: <kind>: <text>` for a message, then on stderr how many it left
//! out (`--all` prints those too, marked `(on <base>)`), and exits 1 on
//! any hit kept, 0 on none,
//! and 2 when it can't look: a word list that is there but can't be read
//! stops it, so a landing asks rather than pushing unchecked.
//! queue-and-clean runs it before a direct landing asks to push, and a
//! landing without asking (autopilot's "Land without asking")
//! asks after all when it finds anything.
//!
//! JavaScript's look-arounds, which Rust's regex hasn't, are the boundary
//! checks of `bounded`, tried again one character on wherever one fails,
//! as a backtracking search would.

use std::collections::{HashMap, HashSet};
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
    found_in(text, test, words)
        .into_iter()
        .map(|(kind, shown, _)| (kind, shown))
        .collect()
}

/// Each hit's kind, the text shown and the whole match (a token's shown
/// cut short).
fn found_in(text: &str, test: bool, words: &[String]) -> Vec<(&'static str, String, String)> {
    let mut hits = Vec::new();
    for w in words {
        let Ok(re) = Regex::new(&format!("(?i){}", regex::escape(w))) else {
            continue;
        };
        let found = bounded(&re, text, |b, a| {
            !b.is_some_and(word) && !a.is_some_and(word)
        });
        if let Some(m) = found.first() {
            hits.push(("listed word", m[0].to_string(), m[0].to_string()));
        }
    }
    if !test {
        let ticket = |b: Option<char>, a: Option<char>| {
            !b.is_some_and(|c| word(c) || c == '-') && !a.is_some_and(word)
        };
        for m in bounded(&TICKET, text, ticket) {
            if !NOT_TICKETS.contains(&&m[1]) {
                hits.push(("ticket ID", m[0].to_string(), m[0].to_string()));
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
                hits.push(("PR number", number.to_string(), number.to_string()));
            }
        }
    }
    for m in HOME_PATH.captures_iter(text) {
        if !PLACEHOLDER_USERS.contains(&m[1].to_lowercase().as_str()) {
            hits.push(("home path", m[0].to_string(), m[0].to_string()));
        }
    }
    let email = |b: Option<char>, a: Option<char>| {
        !b.is_some_and(|c| word(c) || matches!(c, '.' | '+' | '-'))
            && !a.is_some_and(|c| word(c) || c == '-')
    };
    for m in bounded(&EMAIL, text, email) {
        let address = &m[0];
        if !OK_EMAIL.is_match(address) && !(test && OK_TEST_EMAIL.is_match(address)) {
            hits.push(("email address", address.to_string(), address.to_string()));
        }
    }
    let token = |b: Option<char>, _: Option<char>| !b.is_some_and(|c| word(c) || c == '-');
    for m in bounded(&TOKEN, text, token) {
        let shown: String = m[0].chars().take(8).collect();
        hits.push(("token", format!("{shown}…"), m[0].to_string()));
    }
    hits
}

/// One hit: the line printed for it, the file it is in (none for a commit
/// message's), its kind and the whole text it matched.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub shown: String,
    pub file: Option<String>,
    pub kind: &'static str,
    pub whole: String,
}

/// What makes two hits the same: their kind and whole text, a listed word's
/// in any case, as it is found.
fn same(kind: &'static str, whole: &str) -> (&'static str, String) {
    if kind == "listed word" {
        (kind, whole.to_lowercase())
    } else {
        (kind, whole.to_string())
    }
}

/// A path as a diff's `+++` header gives it: the tab git ends a name
/// holding a space with taken off, and a name git quotes (one holding a
/// quote, a backslash or a control character) unquoted.
fn header_path(name: &str) -> String {
    let name = name.strip_suffix('\t').unwrap_or(name);
    let Some(quoted) = name.strip_prefix('"').and_then(|n| n.strip_suffix('"')) else {
        return name.to_string();
    };
    let mut out = String::new();
    let mut chars = quoted.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        out.push(match chars.next() {
            Some('t') => '\t',
            Some('n') => '\n',
            Some(other) => other,
            None => break,
        });
    }
    out
}

/// Every hit in `git log -p -m`'s added lines and in the commit messages
/// `--format=%h%x00%B%x00` printed, each once, in the order found.
pub fn hits(diff: &str, log: &str, words: &[String]) -> Vec<Hit> {
    let mut found: Vec<Hit> = Vec::new();
    let mut file = String::new();
    let mut line = 0usize;
    // A file's headers come before its first hunk; in a hunk, a row that
    // starts `+++ ` is an added line that starts `++ `.
    let mut in_hunk = false;
    for row in diff.split('\n') {
        if row.starts_with("diff --git ") {
            file.clear();
            in_hunk = false;
            continue;
        }
        if let Some(name) = row.strip_prefix("+++ ").filter(|_| !in_hunk) {
            file = if name == "/dev/null" {
                String::new()
            } else {
                let name = header_path(name);
                name.strip_prefix("b/").unwrap_or(&name).to_string()
            };
            continue;
        }
        if let Some(m) = HUNK.captures(row) {
            line = m[1].parse().unwrap_or(0);
            in_hunk = true;
            continue;
        }
        let Some(added) = row.strip_prefix('+') else {
            continue;
        };
        if file.is_empty() {
            continue;
        }
        for (kind, text, whole) in found_in(added, is_test(&file), words) {
            found.push(Hit {
                shown: format!("{file}:{line}: {kind}: {text}"),
                file: Some(file.clone()),
                kind,
                whole,
            });
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
        for (kind, text, whole) in found_in(&message.join("\n"), false, words) {
            found.push(Hit {
                shown: format!("commit {hash}: {kind}: {text}"),
                file: None,
                kind,
                whole,
            });
        }
    }
    let mut seen = HashSet::new();
    // A token is shown cut short, so two on a line differ only whole.
    found.retain(|hit| seen.insert((hit.shown.clone(), hit.whole.clone())));
    found
}

/// The hits to report and those the base already has: a file's hit is left
/// out when the same hit, found by the same rules, is in `base_copy` (the
/// base's copy of the file, none when it has none), so `ABC-123` there
/// leaves out no `ABC-12`; a commit message's never is.
pub fn leave_out(
    found: Vec<Hit>,
    words: &[String],
    mut base_copy: impl FnMut(&str) -> Option<String>,
) -> (Vec<Hit>, Vec<Hit>) {
    let mut on_base: HashMap<String, HashSet<(&'static str, String)>> = HashMap::new();
    found.into_iter().partition(|hit| {
        let Some(file) = &hit.file else {
            return true;
        };
        let had = on_base.entry(file.clone()).or_insert_with(|| {
            let copy = base_copy(file).unwrap_or_default();
            copy.lines()
                .flat_map(|line| found_in(line, is_test(file), words))
                .map(|(kind, _, whole)| same(kind, &whole))
                .collect()
        });
        !had.contains(&same(hit.kind, &hit.whole))
    })
}

/// The commit an `a..b` range starts from, the merge base of its ends;
/// none for a single revision, whose log reads all that came before it, or
/// when there is none.
fn base_of(range: &str) -> Option<String> {
    let end = |s: &str| {
        if s.is_empty() {
            "HEAD".to_string()
        } else {
            s.to_string()
        }
    };
    let (a, b) = range.split_once("..")?;
    let b = b.strip_prefix('.').unwrap_or(b);
    git(&["merge-base", &end(a), &end(b)])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
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
    let all = args.iter().any(|a| a == "--all");
    let rest: Vec<&String> = args.iter().filter(|a| *a != "--all").collect();
    let [range] = rest.as_slice() else {
        eprintln!("usage: leak-check [--all] <range>");
        return ExitCode::from(2);
    };
    let range = range.as_str();
    let words = match read_words(&words_file()) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("leak-check: {e}");
            return ExitCode::from(2);
        }
    };
    let looked = git(&[
        "-c",
        "core.quotePath=false",
        "log",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--text",
        "--unified=0",
        "--no-renames",
        "--src-prefix=a/",
        "--dst-prefix=b/",
        "--no-relative",
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
    let base = base_of(range);
    let (kept, left) = leave_out(hits(&diff, &log, &words), &words, |file| {
        let base = base.as_deref()?;
        git(&["show", "--no-textconv", &format!("{base}:{file}")]).ok()
    });
    for hit in &kept {
        println!("{}", hit.shown);
    }
    if !left.is_empty() {
        let short: String = base.as_deref().unwrap_or("").chars().take(8).collect();
        if all {
            for hit in &left {
                println!("{} (on {short})", hit.shown);
            }
        }
        eprintln!("leak-check: {} already on {short} left out", left.len());
    }
    ExitCode::from(if kept.is_empty() { 0 } else { 1 })
}
