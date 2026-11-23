//! The file paths a pane's text names, for "Open a file from this pane in
//! the side panel": a path as Claude, a compiler or a test runner writes
//! one (`src/mux/server.rs`, `./build.rs`, `~/Notes/Todo.md`,
//! `/etc/hosts`), with the line and column after it when given
//! (`src/mux/server.rs:3385`, `main.rs:12:4`), newest first, each path once
//! at its newest mention. New, with no tmux-version file behind it.
//!
//! Only the text is read here, so it runs on the server's task over what
//! the pane already holds; whether a path is a file is asked off it
//! (`resolve`), and the repository's files (`repo_files`) likewise.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use regex::Regex;

/// A path a pane names, as written, and the line and column after it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub path: String,
    pub line: Option<usize>,
    pub col: Option<usize>,
}

impl Found {
    /// The path as `switchboard code` takes it: `path:line`.
    pub fn arg(&self, file: &Path) -> String {
        // Always with a line, so a file whose name ends in `:<digits>`
        // isn't read as one.
        format!("{}:{}", file.display(), self.line.unwrap_or(1))
    }

    /// What its row in the list says: the path and where in it.
    pub fn row(&self) -> String {
        match (self.line, self.col) {
            (Some(l), Some(c)) => format!("{}:{l}:{c}", self.path),
            (Some(l), None) => format!("{}:{l}", self.path),
            _ => self.path.clone(),
        }
    }
}

/// A path with a line (or a range of lines, the first taken) and a column
/// after it, the whole of a word.
static PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?P<path>~?/?(?:[\w.@+-]+/)*[\w.@+-]+)(?::(?P<line>\d+)(?:-\d+)?)?(?::(?P<col>\d+))?$",
    )
    .expect("a regex")
});

/// A file name with an extension: a dot inside it, a letter after it.
static NAMED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\w.@+-]*[\w@+-]\.[A-Za-z]\w*$").expect("a regex"));

/// Whether a character can be part of a path with its line and column.
fn in_word(c: char) -> bool {
    c.is_alphanumeric() || "._/~@+-:".contains(c)
}

/// The paths `text` names, newest (lowest on the screen, rightmost on its
/// line) first, each once with the line of its newest mention.
pub fn paths_in(text: &str) -> Vec<Found> {
    let mut found: Vec<Found> = Vec::new();
    for line in text.lines().rev() {
        let words: Vec<&str> = line.split(|c: char| !in_word(c)).collect();
        for word in words.into_iter().rev() {
            // A sentence's full stop or colon after a path isn't its own.
            let word = word.trim_end_matches(['.', ':']);
            if word.contains("://") || word.starts_with("//") {
                continue;
            }
            let Some(m) = PATH.captures(word) else {
                continue;
            };
            let path = m["path"].to_string();
            // A word with no folder is a file only by its extension, and a
            // version or a number ("0.11.0") is none.
            let name = path.rsplit('/').next().unwrap_or_default();
            let looks_like_one = path.contains('/') || NAMED.is_match(name);
            if !looks_like_one || name.chars().all(|c| c.is_ascii_digit() || c == '.') {
                continue;
            }
            if found.iter().any(|f| f.path == path) {
                continue;
            }
            let number = |name: &str| m.name(name).and_then(|n| n.as_str().parse().ok());
            found.push(Found {
                path,
                line: number("line").filter(|&l| l > 0),
                col: number("col"),
            });
        }
    }
    found
}

/// The file a path names from `dir`: as written from it, `~/` from `home`,
/// an absolute path as it is, else from the top of the repository `dir` is
/// in, where a path in a diff or a test's output starts; `None` when it is
/// no file. Reads the file system, so never on the server's task.
pub fn resolve(path: &str, dir: &Path, home: &Path) -> Option<PathBuf> {
    let file = if let Some(rest) = path.strip_prefix("~/") {
        home.join(rest)
    } else {
        dir.join(path)
    };
    if file.is_file() {
        return Some(file);
    }
    let top = dir.ancestors().find(|d| d.join(".git").exists())?;
    let file = top.join(path);
    file.is_file().then_some(file)
}

/// The repository's files as `repo_files` lists them.
pub struct RepoFiles {
    /// The top of the repository.
    pub top: PathBuf,
    /// Its files, by their path from the top.
    pub files: Vec<String>,
    /// Whether git listed more than were taken, or took too long.
    pub cut: bool,
}

/// The top of the repository `dir` is in, and its files as git lists them
/// (tracked, and untracked but not ignored), relative to that top, from a
/// pane in a folder of it too; none outside a repository. At most `most`
/// of them, read as git prints them, and only what it printed within
/// `within`, git ended there, since a repository at the home folder or on a
/// slow disk can take minutes to walk. Starts git, so never on the server's
/// task.
pub fn repo_files(dir: &Path, most: usize, within: Duration) -> Option<RepoFiles> {
    let top = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let top = PathBuf::from(String::from_utf8_lossy(&top.stdout).trim_end_matches('\n'));
    let mut git = Command::new("git")
        .arg("-C")
        .arg(&top)
        .args(["-c", "core.quotePath=false"])
        .args(["ls-files", "--cached", "--others", "--exclude-standard"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = git.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        // By bytes, a name that isn't UTF-8 (git prints names raw with
        // quotePath off) read as near as it can be rather than ending the list.
        let mut reader = BufReader::new(stdout);
        let mut line = Vec::new();
        while reader.read_until(b'\n', &mut line).is_ok_and(|n| n > 0) {
            let name = String::from_utf8_lossy(&line)
                .trim_end_matches('\n')
                .to_string();
            line.clear();
            if tx.send(name).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + within;
    let mut files: Vec<String> = Vec::new();
    let mut cut = false;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(_) if files.len() >= most => {
                cut = true;
                break;
            }
            Ok(line) => files.push(line),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                cut = true;
                break;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = git.kill();
    let _ = git.wait();
    files.dedup();
    Some(RepoFiles { top, files, cut })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(text: &str) -> Vec<String> {
        paths_in(text).into_iter().map(|f| f.row()).collect()
    }

    #[test]
    fn a_path_with_its_line_and_column_is_found_newest_first() {
        let text = "Read src/lib.rs first.\n\
                    error[E0308]: mismatched types\n  --> crates/ui/src/kit.rs:12:5\n\
                    See `src/mux/server.rs:3385`, and Cargo.toml.";
        assert_eq!(
            paths(text),
            [
                "Cargo.toml",
                "src/mux/server.rs:3385",
                "crates/ui/src/kit.rs:12:5",
                "src/lib.rs"
            ]
        );
    }

    #[test]
    fn each_path_is_listed_once_at_its_newest_mention() {
        let text = "src/a.rs:1\nsrc/b.rs\nsrc/a.rs:40-52";
        assert_eq!(
            paths(text),
            ["src/a.rs:40", "src/b.rs"],
            "a range from its first line"
        );
    }

    #[test]
    fn links_versions_and_words_are_no_paths() {
        let text = "https://github.com/a/b.rs v0.11.0 1.5 hello done.";
        assert_eq!(paths(text), Vec::<String>::new());
    }

    #[test]
    fn home_relative_dotted_and_extensionless_paths_are_found() {
        let text = "~/Notes/Todo.md ./build.rs ../x/y.ts .githooks/pre-push /etc/hosts";
        assert_eq!(
            paths(text),
            [
                "/etc/hosts",
                ".githooks/pre-push",
                "../x/y.ts",
                "./build.rs",
                "~/Notes/Todo.md"
            ]
        );
    }

    #[test]
    fn a_path_is_resolved_from_the_pane_then_the_repository_and_one_missing_is_dropped() {
        let dir = std::env::temp_dir().join(format!("sb-code-find-{}", std::process::id()));
        let sub = dir.join("crates/ui");
        std::fs::create_dir_all(sub.join("src")).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(sub.join("src/kit.rs"), "").unwrap();
        std::fs::write(dir.join("Cargo.toml"), "").unwrap();
        let home = dir.join("home");
        assert_eq!(
            resolve("src/kit.rs", &sub, &home),
            Some(sub.join("src/kit.rs"))
        );
        assert_eq!(
            resolve("Cargo.toml", &sub, &home),
            Some(dir.join("Cargo.toml")),
            "from the repository's top"
        );
        assert_eq!(resolve("src/gone.rs", &sub, &home), None);
        assert_eq!(
            resolve("crates/ui", &dir, &home),
            None,
            "a folder is no file"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
