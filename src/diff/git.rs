//! What M-d's diff reads from git: what a branch is compared with (ported
//! from tmux/diff-base.sh), and the change itself: its files with their
//! status and changed lines, the diff as the story reads it, the commits'
//! messages, and each file's text on either side.
//!
//! The base is the branch's PR's base branch (origin's default branch
//! without a PR, main where origin doesn't say), as origin has it now, so it
//! is fetched first: a base last fetched before the branch was rebased onto
//! a newer one would count that newer work as the branch's. M-d and the
//! story told ahead share it, so both compare with the same base and so tell
//! the same story. The cache holds, per worktree, the commit last compared
//! and its base: the merge base can't move while the branch stays at that
//! commit, so neither the lookup nor the fetch is needed again until it
//! moves (`~/.cache/switchboard/diff-base/`).
//!
//! The change is GitHub's "Files changed": the branch's commits since it
//! left its base, `git diff <merge base> HEAD`, committed changes only.
//!
//! Asked for, the change is instead the working tree's, as diffview's
//! plain `:DiffviewOpen` showed it: the changes staged (HEAD against the
//! index) and those not (the index against the files, files git doesn't
//! know yet among them as added), each file listed in its own section, and
//! the story read from both, the staged first, as diffview.lua's
//! `read_diff` ran them.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

use anyhow::{Result, anyhow, bail};
use regex::Regex;

use crate::{paths, util};

/// Runs git in `dir` with its output as text: an error with the first line
/// of what it said when it fails.
pub fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).args(args);
    let out = util::output_of(&mut cmd)?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// As `git`, its output's bytes as they are (a file's text may not be UTF-8).
pub fn git_bytes(dir: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

/// Whether git knows `rev` here.
fn verifies(dir: &Path, rev: &str) -> bool {
    git(dir, &["rev-parse", "--verify", "--quiet", rev]).is_ok()
}

/// The repository's top directory, its git directory and the commit HEAD is
/// at.
pub struct Facts {
    pub top: PathBuf,
    pub git_dir: PathBuf,
    pub head: String,
}

pub fn facts(dir: &Path) -> Result<Facts> {
    let out = git(
        dir,
        &["rev-parse", "--absolute-git-dir", "--show-toplevel", "HEAD"],
    )
    .map_err(|_| anyhow!("Not in a git repository: {}", paths::tilde(dir)))?;
    let mut lines = out.lines();
    let (Some(git_dir), Some(top), Some(head)) = (lines.next(), lines.next(), lines.next()) else {
        bail!(
            "Not in a git repository with a commit: {}",
            paths::tilde(dir)
        );
    };
    Ok(Facts {
        top: PathBuf::from(top),
        git_dir: PathBuf::from(git_dir),
        head: head.trim().to_string(),
    })
}

/// A branch name as it may go on a command line: only a branch's characters.
static SAFE_REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9._/-]+$").unwrap());

pub fn safe_ref(name: &str) -> bool {
    SAFE_REF.is_match(name)
}

fn base_cache(git_dir: &Path) -> PathBuf {
    paths::cache_dir()
        .join("diff-base")
        .join(git_dir.to_string_lossy().replace('/', "%"))
}

/// The base kept for the branch's commit, while it is still there; nothing
/// asked of the network (diff-base.sh's `cached`).
pub fn cached_base(dir: &Path) -> Option<String> {
    let facts = facts(dir).ok()?;
    let text = util::read_text(base_cache(&facts.git_dir))?;
    let (head, base) = text.trim_end().split_once('\t')?;
    (head == facts.head && safe_ref(base) && verifies(dir, base)).then(|| base.to_string())
}

/// The base asked for: the ref to compare with, and whether it was fetched
/// just now or has nowhere to be fetched from (else it is as last fetched).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Base {
    pub base: String,
    pub fetched: bool,
}

/// The PR's base branch as GitHub has it, `None` with no PR, GitHub off or
/// out of reach.
fn pr_base(dir: &Path) -> Option<String> {
    if !crate::settings::is_on("github") {
        return None;
    }
    let dir = dir.to_path_buf();
    util::block_on(async move {
        crate::integrations::github::pr_for_branch(&dir, "M-d diff: PR base")
            .await
            .ok()
            .flatten()
            .map(|pr| pr.base_ref_name)
    })
    .filter(|b| !b.is_empty())
}

/// The base where neither the PR nor origin says: main, or master in a
/// repository that has it and no main, as one made by a git older than
/// `init.defaultBranch`.
fn default_base(dir: &Path) -> &'static str {
    ["main", "master"]
        .into_iter()
        .find(|b| verifies(dir, &format!("origin/{b}")) || verifies(dir, b))
        .unwrap_or("main")
}

/// Asks GitHub for the PR's base and fetches it, keeping the answer for
/// the branch's commit (diff-base.sh's `fetch`). `None` outside a
/// repository.
pub fn fetch_base(dir: &Path) -> Option<Base> {
    let facts = facts(dir).ok()?;
    let mut base = pr_base(dir)
        .or_else(|| {
            git(dir, &["rev-parse", "--abbrev-ref", "origin/HEAD"])
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_default();
    if let Some(rest) = base.strip_prefix("origin/") {
        base = rest.to_string();
    }
    if base.is_empty() || !safe_ref(&base) {
        base = default_base(dir).into();
    }
    // A repository with no origin has nothing to fetch, its own branch as
    // fresh as it gets, so nothing is asked and nothing said.
    let origin = git(dir, &["config", "--get", "remote.origin.url"]).is_ok();
    let fetched = !origin || {
        crate::worktree::count_fetch(&facts.top, "origin", "M-d diff: fetch base");
        git(dir, &["fetch", "--quiet", "origin", &base]).is_ok()
    };
    // A repository with no origin compares with its own branch.
    let mut found = format!("origin/{base}");
    if !verifies(dir, &found) {
        found = base;
    }
    // Only a comparison with a freshly fetched base is worth keeping.
    if fetched {
        let _ = util::write_atomic(
            base_cache(&facts.git_dir),
            format!("{}\t{found}\n", facts.head),
        );
    }
    Some(Base {
        base: found,
        fetched,
    })
}

/// Which list a changed file is in: a branch's committed change (M-d's),
/// or a working tree's changes not staged and staged, which diffview listed
/// apart, in that order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Section {
    #[default]
    Committed,
    Unstaged,
    Staged,
}

impl Section {
    /// The list's heading.
    pub fn heading(self) -> &'static str {
        match self {
            Section::Committed | Section::Unstaged => "Changes",
            Section::Staged => "Staged changes",
        }
    }
}

/// The index's side of a file, as `git show :<path>` reads it.
pub const INDEX: &str = "";
/// The file in the working tree, read from disk.
pub const WORK_TREE: &str = "(work tree)";

/// One changed file: its path (the new one for a rename) and the old one,
/// its status letter as git gives it (A, D, M, R, C, T), its changed line
/// counts, whether git calls it binary, and the list it is in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub old_path: Option<String>,
    pub status: char,
    pub additions: usize,
    pub deletions: usize,
    pub binary: bool,
    pub section: Section,
}

impl Entry {
    /// What tells it from the others: its path, and its section in a
    /// working tree, where one file may be both staged and changed since.
    pub fn key(&self) -> String {
        match self.section {
            Section::Committed => self.path.clone(),
            other => format!("{other:?}:{}", self.path),
        }
    }

    /// Its path on the old side.
    pub fn old(&self) -> &str {
        self.old_path.as_deref().unwrap_or(&self.path)
    }

    /// Whether it is shown alone on one side: an added file, or a deleted
    /// one as it was.
    pub fn one_sided(&self) -> bool {
        matches!(self.status, 'A' | 'D')
    }
}

/// What M-d shows: the repository, what it compares (the base's name, the
/// merge base and the branch's commit), the files, the diff as the story
/// reads it, and the commits' messages as `story::commits` reads them. A
/// working tree's has HEAD on both sides, its staged changes' diff in
/// `staged` and its changes not staged in `diff`, and no commits.
#[derive(Clone, Debug, Default)]
pub struct Change {
    pub top: PathBuf,
    pub base: String,
    pub left: String,
    pub right: String,
    pub entries: Vec<Entry>,
    pub diff: String,
    pub staged: Option<String>,
    pub raw_commits: String,
}

impl Change {
    /// Whether it is a working tree's.
    pub fn working(&self) -> bool {
        self.staged.is_some()
    }

    /// The diff's runs, as `story::split` reads them: the staged changes
    /// first in a working tree's.
    pub fn runs(&self) -> Vec<&str> {
        match &self.staged {
            Some(staged) => vec![staged.as_str(), self.diff.as_str()],
            None => vec![self.diff.as_str()],
        }
    }

    /// What a file's two sides are read from (`text_at`): the commits
    /// compared, else HEAD and the index for a staged file, the index and
    /// the working tree for one not staged.
    pub fn sides(&self, entry: &Entry) -> (String, String) {
        match entry.section {
            Section::Committed => (self.left.clone(), self.right.clone()),
            Section::Staged => (self.right.clone(), INDEX.into()),
            Section::Unstaged => (INDEX.into(), WORK_TREE.into()),
        }
    }

    /// The commit a side's GitHub link names: HEAD for the index or the
    /// working tree, which GitHub doesn't have.
    pub fn commit_of<'a>(&'a self, side: &'a str) -> &'a str {
        if side == INDEX || side == WORK_TREE {
            &self.right
        } else {
            side
        }
    }
}

/// git's own a/ and b/ whatever the config says, which `story::split`
/// reads, and never the repository's text converters or external tools.
const PLAIN: &[&str] = &[
    "-c",
    "diff.noprefix=false",
    "-c",
    "core.quotePath=false",
    "diff",
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
];

/// The files `git diff -z --name-status` and `--numstat` list, put together.
pub fn parse_entries(name_status: &str, numstat: &str) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut fields = name_status.split('\0').filter(|f| !f.is_empty());
    while let Some(status) = fields.next() {
        let letter = status.chars().next().unwrap_or('M');
        let (old_path, path) = if matches!(letter, 'R' | 'C') {
            let old = fields.next().unwrap_or("").to_string();
            (Some(old), fields.next().unwrap_or("").to_string())
        } else {
            (None, fields.next().unwrap_or("").to_string())
        };
        if path.is_empty() {
            continue;
        }
        entries.push(Entry {
            path,
            old_path,
            status: letter,
            additions: 0,
            deletions: 0,
            binary: false,
            section: Section::Committed,
        });
    }
    // numstat -z: "added\tdeleted\tpath\0", or for a rename
    // "added\tdeleted\t\0old\0new\0"; "-\t-" for a binary file.
    let mut fields = numstat.split('\0');
    while let Some(head) = fields.next() {
        let mut parts = head.splitn(3, '\t');
        let (Some(added), Some(deleted), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let path = if path.is_empty() {
            let _old = fields.next();
            fields.next().unwrap_or("").to_string()
        } else {
            path.to_string()
        };
        if let Some(e) = entries.iter_mut().find(|e| e.path == path) {
            e.binary = added == "-";
            e.additions = added.parse().unwrap_or(0);
            e.deletions = deleted.parse().unwrap_or(0);
        }
    }
    entries
}

/// The change between `base`'s merge base with HEAD and HEAD, in the
/// repository at `top`, as `DiffviewOpen <base>...HEAD` compared them.
pub fn read_change(top: &Path, base: &str) -> Result<Change> {
    let right = git(top, &["rev-parse", "HEAD"])?.trim().to_string();
    let left = git(top, &["merge-base", base, "HEAD"])
        .map_err(|_| anyhow!("{base} and this branch have no commit in common"))?
        .trim()
        .to_string();
    read_range(top, base, left, right)
}

/// The commits between a branch's merge base and its head, oldest first,
/// each its hash and subject, as diffview's history listed them with
/// `--reverse`.
pub fn commits_between(top: &Path, left: &str, right: &str) -> Vec<(String, String)> {
    git(
        top,
        &[
            "log",
            "--reverse",
            "--no-color",
            "--format=%H%x00%s",
            &format!("{left}..{right}"),
        ],
    )
    .unwrap_or_default()
    .lines()
    .filter_map(|l| {
        let (hash, subject) = l.split_once('\0')?;
        Some((hash.to_string(), subject.to_string()))
    })
    .collect()
}

/// One commit's own change, against its first parent (the empty tree for
/// a first commit), as diffview's history showed each.
pub fn read_commit(top: &Path, commit: &str) -> Result<Change> {
    if !safe_ref(commit) {
        bail!("Not a commit: {commit}");
    }
    let right = git(
        top,
        &["rev-parse", "--verify", &format!("{commit}^{{commit}}")],
    )
    .map_err(|_| anyhow!("No commit {commit} here"))?
    .trim()
    .to_string();
    let left = match git(
        top,
        &["rev-parse", "--verify", "--quiet", &format!("{right}^")],
    ) {
        Ok(parent) => parent.trim().to_string(),
        Err(_) => git(top, &["hash-object", "-t", "tree", "/dev/null"])?
            .trim()
            .to_string(),
    };
    read_range(top, &right, left, right.clone())
}

/// The change from `left` to `right`, what M-d and a commit of the history
/// both show.
fn read_range(top: &Path, base: &str, left: String, right: String) -> Result<Change> {
    let range = |extra: &[&str]| -> Result<String> {
        let mut args: Vec<&str> = PLAIN.to_vec();
        args.extend_from_slice(extra);
        args.extend_from_slice(&[left.as_str(), right.as_str()]);
        git(top, &args)
    };
    let name_status = range(&["-M", "-z", "--name-status"])?;
    let numstat = range(&["-M", "-z", "--numstat"])?;
    let entries = parse_entries(&name_status, &numstat);
    let diff = range(&["--src-prefix=a/", "--dst-prefix=b/", "-U2", "-M"])?;
    let raw_commits = git(
        top,
        &[
            "log",
            "--no-color",
            "--format=%H%x00%s%x00%b%x1e",
            &format!("{left}..{right}"),
        ],
    )
    .unwrap_or_default();
    Ok(Change {
        top: top.to_path_buf(),
        base: base.to_string(),
        left,
        right,
        entries,
        diff,
        staged: None,
        raw_commits,
    })
}

/// The lines of a file git doesn't know yet that the story reads, as an
/// addition of its first lines, as many as diffstory.lua's `LINES_PER_FILE`.
const NEW_FILE_LINES: usize = 400;

/// The working tree's changes in the repository at `top`: those staged and
/// those not, files git doesn't know yet among the second as added, which
/// its diff leaves out (diffview.lua's `read_diff` for plain
/// `:DiffviewOpen`).
pub fn read_working(top: &Path) -> Result<Change> {
    let head = git(top, &["rev-parse", "HEAD"])
        .map_err(|_| anyhow!("This repository has no commit yet"))?
        .trim()
        .to_string();
    let run = |extra: &[&str]| -> Result<String> {
        let mut args: Vec<&str> = PLAIN.to_vec();
        args.extend_from_slice(extra);
        git(top, &args)
    };
    let mut entries = Vec::new();
    let mut texts = Vec::new();
    for (section, cached) in [(Section::Staged, true), (Section::Unstaged, false)] {
        let with = |extra: &[&str]| -> Result<String> {
            let mut args: Vec<&str> = if cached { vec!["--cached"] } else { Vec::new() };
            args.extend_from_slice(extra);
            run(&args)
        };
        let listed = parse_entries(
            &with(&["-M", "-z", "--name-status"])?,
            &with(&["-M", "-z", "--numstat"])?,
        );
        entries.extend(listed.into_iter().map(|e| Entry { section, ..e }));
        texts.push(with(&["--src-prefix=a/", "--dst-prefix=b/", "-U2", "-M"])?);
    }
    let untracked = git(top, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    for path in untracked.split('\0').filter(|p| !p.is_empty()) {
        let Some(text) = text_at(top, WORK_TREE, path) else {
            entries.push(Entry {
                path: path.to_string(),
                old_path: None,
                status: 'A',
                additions: 0,
                deletions: 0,
                binary: true,
                section: Section::Unstaged,
            });
            continue;
        };
        let lines: Vec<&str> = text.lines().collect();
        let shown = &lines[..lines.len().min(NEW_FILE_LINES)];
        let unstaged = texts.last_mut().expect("two runs");
        unstaged.push_str(&format!(
            "diff --git a/{path} b/{path}\nnew file\n@@ -0,0 +1,{} @@\n",
            shown.len()
        ));
        for line in shown {
            unstaged.push('+');
            unstaged.push_str(line);
            unstaged.push('\n');
        }
        entries.push(Entry {
            path: path.to_string(),
            old_path: None,
            status: 'A',
            additions: lines.len(),
            deletions: 0,
            binary: false,
            section: Section::Unstaged,
        });
    }
    let diff = texts.pop().unwrap_or_default();
    Ok(Change {
        top: top.to_path_buf(),
        base: String::new(),
        left: head.clone(),
        right: head,
        entries,
        diff,
        staged: texts.pop(),
        raw_commits: String::new(),
    })
}

/// A file's text at a commit (`INDEX` for the index's, `WORK_TREE` for the
/// file on disk, a link as the path it points to, as git records one),
/// `None` when it isn't there or isn't text.
pub fn text_at(top: &Path, commit: &str, path: &str) -> Option<String> {
    let bytes = if commit == WORK_TREE {
        let file = top.join(path);
        let meta = std::fs::symlink_metadata(&file).ok()?;
        if meta.file_type().is_symlink() {
            std::fs::read_link(&file)
                .ok()?
                .to_string_lossy()
                .into_owned()
                .into_bytes()
        } else if meta.is_file() {
            std::fs::read(&file).ok()?
        } else {
            return None;
        }
    } else {
        git_bytes(top, &["show", &format!("{commit}:{path}")])?
    };
    if bytes.iter().take(8000).any(|b| *b == 0) {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// The GitHub repository `origin` names, as `owner/name`.
pub fn github_repo(top: &Path) -> Option<String> {
    let url = git(top, &["remote", "get-url", "origin"]).ok()?;
    crate::integrations::github::parse_remote(url.trim()).map(|r| r.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_status_and_numstat_make_one_list_renames_and_binaries_included() {
        let names = "M\0src/a.js\0R087\0old/b.js\0new/b.js\0A\0img.png\0D\0gone.txt\0";
        let counts = [
            "3\t1\tsrc/a.js",
            "2\t2\t",
            "old/b.js",
            "new/b.js",
            "-\t-\timg.png",
            "0\t4\tgone.txt",
            "",
        ]
        .join("\0");
        let entries = parse_entries(names, &counts);
        assert_eq!(entries.len(), 4);
        assert_eq!((entries[0].additions, entries[0].deletions), (3, 1));
        assert_eq!(entries[1].path, "new/b.js");
        assert_eq!(entries[1].old(), "old/b.js");
        assert_eq!(entries[1].status, 'R');
        assert_eq!((entries[1].additions, entries[1].deletions), (2, 2));
        assert!(entries[2].binary && entries[2].one_sided());
        assert!(entries[3].one_sided() && entries[3].deletions == 4);
    }

    #[test]
    fn a_repository_with_master_and_no_main_compares_with_master() {
        let dir = std::env::temp_dir().join(format!("sb-diff-base-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(["-c", "user.name=T", "-c", "user.email=t@example.com"])
                .args([
                    "-c",
                    "commit.gpgsign=false",
                    "-c",
                    "core.hooksPath=/dev/null",
                ])
                .args(args)
                .current_dir(&dir)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        run(&["init", "-q"]);
        run(&["checkout", "-q", "-b", "master"]);
        run(&["commit", "-q", "--allow-empty", "-m", "first"]);
        run(&["checkout", "-q", "-b", "feature"]);
        assert_eq!(default_base(&dir), "master");
        run(&["branch", "-q", "main"]);
        assert_eq!(default_base(&dir), "main");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_a_branch_names_characters_go_on_a_command_line() {
        assert!(safe_ref("origin/feature-x_1.2"));
        assert!(!safe_ref("main; rm -rf ~"));
        assert!(!safe_ref(""));
    }
}
