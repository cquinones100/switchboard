//! What M-d's diff reads from git: what a branch is compared with (ported
//! from tmux/diff-base.sh), and the change itself: its files with their
//! status and changed lines, the diff as the story reads it, the commits'
//! messages, and each file's text on either side.
//!
//! The base is the branch's PR's base branch (origin's default branch
//! without a PR, main where origin doesn't say), as origin has it now, so it
//! is fetched first. On the default branch itself that is its own copy on
//! origin, so the change is its commits not yet pushed; a branch with no PR
//! whose upstream is another branch (one cut from a feature branch) compares
//! with that branch instead. What the base is to the branch (`Why`) is kept
//! with it, for the diff's header to say. It is fetched first because a base last fetched before the branch was rebased onto
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
//! With the diff_uncommitted setting on (diffview.lua's `uncommitted_mode`
//! and `look_uncommitted`, and pane-in-place.sh, as upstream's 7b85aa7
//! changed them), M-d's change runs from the merge base to the working tree
//! instead, clean or not, so edits made while the diff is open come in when
//! it is read again: the branch's commits and its changes not yet committed
//! together, files git doesn't know yet among them as added, with which
//! files have changes not yet committed (staged, unstaged or untracked) and
//! the committed part beside it, whose story is the one told
//! (`read_to_work_tree`).
//!
//! Asked for, the change is instead the working tree's, as diffview's
//! plain `:DiffviewOpen` showed it: the changes staged (HEAD against the
//! index) and those not (the index against the files, files git doesn't
//! know yet among them as added), each file listed in its own section, and
//! the story read from both, the staged first, as diffview.lua's
//! `read_diff` ran them.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Result, anyhow, bail};
use regex::Regex;
use switchboard_plugin_api::Pr;
use switchboard_plugin_api::sdk::{git_as_is, git_bytes};

use crate::{paths, util};

/// Whether git knows `rev` here.
fn verifies(dir: &Path, rev: &str) -> bool {
    git_as_is(dir, &["rev-parse", "--verify", "--quiet", rev]).is_ok()
}

/// The repository's top directory, its git directory and the commit HEAD is
/// at.
pub struct Facts {
    pub top: PathBuf,
    pub git_dir: PathBuf,
    pub head: String,
}

pub fn facts(dir: &Path) -> Result<Facts> {
    let out = git_as_is(
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

/// Forgets the base kept for a worktree (by its git directory), so the next
/// M-d asks for it again; whether one was kept.
pub fn forget_base(git_dir: &Path) -> bool {
    std::fs::remove_file(base_cache(git_dir)).is_ok()
}

/// What the base is to the branch, which the diff's header says beside it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Why {
    /// Named by whoever asked (`switchboard diff --base`), or a commit of
    /// the history's parent: nothing to add.
    #[default]
    Named,
    /// The base branch of the branch's PR, by its number.
    Pr(u64),
    /// The branch's own copy on origin: the default branch itself, whose
    /// change is its commits not yet pushed.
    Own,
    /// The default branch, or the branch's upstream, for a branch with no
    /// PR.
    NoPr,
}

impl Why {
    /// As the base's cache keeps it.
    fn word(&self) -> String {
        match self {
            Why::Named => "named".into(),
            Why::Pr(number) => format!("pr:{number}"),
            Why::Own => "own".into(),
            Why::NoPr => "none".into(),
        }
    }

    fn from_word(word: &str) -> Option<Why> {
        match word {
            "named" => Some(Why::Named),
            "own" => Some(Why::Own),
            "none" => Some(Why::NoPr),
            other => other.strip_prefix("pr:")?.parse().ok().map(Why::Pr),
        }
    }
}

/// The base kept for the branch's commit, while it is still there; nothing
/// asked of the network (diff-base.sh's `cached`).
pub fn cached_base(dir: &Path) -> Option<String> {
    cached(dir).map(|(base, _)| base)
}

/// As `cached_base`, with what the base is to the branch. It is kept for
/// the branch and its upstream too, which the base follows: another branch
/// checked out at the same commit, or the same one given another upstream,
/// asks again. A line an older build kept, which says neither, counts as
/// none kept.
pub fn cached(dir: &Path) -> Option<(String, Why)> {
    let facts = facts(dir).ok()?;
    let text = util::read_text(base_cache(&facts.git_dir))?;
    let mut fields = text.trim_end_matches('\n').split('\t');
    let (head, base, why, tracking) = (
        fields.next()?,
        fields.next()?,
        fields.next()?,
        fields.next()?,
    );
    let why = Why::from_word(why)?;
    let still_there = head == facts.head && safe_ref(base) && verifies(dir, base);
    (still_there && tracking == tracking_of(dir, current_branch(dir).as_deref()))
        .then(|| (base.to_string(), why))
}

/// How long a base kept as a branch's with no PR is taken as it is: a PR
/// opened since, at the same commit, shows once it is older.
const NO_PR_KEPT: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Whether a base kept that long ago may have a PR by now: only one kept
/// as a branch's with no PR, and only while GitHub is asked at all.
fn pr_may_have_come(why: &Why, kept_for: std::time::Duration, github: bool) -> bool {
    github && *why == Why::NoPr && kept_for > NO_PR_KEPT
}

/// `cached` for M-d opening, which says what the base is to the branch: a
/// base kept as a branch's with no PR counts as none kept once it is ten
/// minutes old, so GitHub is asked again and a PR opened since is named,
/// and its base taken. The story's own readers keep `cached_base`, whose
/// base they need whatever its label.
pub fn cached_for_opening(dir: &Path) -> Option<(String, Why)> {
    let (base, why) = cached(dir)?;
    let kept_for = facts(dir)
        .ok()
        .and_then(|f| std::fs::metadata(base_cache(&f.git_dir)).ok())
        .and_then(|m| m.modified().ok())
        .and_then(|at| at.elapsed().ok())
        .unwrap_or_default();
    (!pr_may_have_come(&why, kept_for, crate::settings::is_on("github"))).then_some((base, why))
}

/// The branch checked out in `dir`, `None` on none.
fn current_branch(dir: &Path) -> Option<String> {
    git_as_is(dir, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// A branch's upstream and its remote's name, as git's config has them:
/// both empty with none, the remote "." or empty for a local branch.
fn upstream_of(dir: &Path, branch: &str) -> (String, String) {
    let out = git_as_is(
        dir,
        &[
            "for-each-ref",
            "--format=%(upstream:short)%09%(upstream:remotename)",
            &format!("refs/heads/{branch}"),
        ],
    )
    .unwrap_or_default();
    let (upstream, remote) = out
        .lines()
        .next()
        .unwrap_or("")
        .split_once('\t')
        .unwrap_or(("", ""));
    (upstream.to_string(), remote.to_string())
}

/// What the base's cache is kept for beside the commit: the branch and its
/// upstream, a space between them (a branch's name holds none).
fn tracking_of(dir: &Path, branch: Option<&str>) -> String {
    match branch {
        Some(branch) => format!("{branch} {}", upstream_of(dir, branch).0),
        None => String::new(),
    }
}

/// The base asked for: the ref to compare with, what it is to the branch,
/// and whether it was fetched just now or has nowhere to be fetched from
/// (else it is as last fetched).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Base {
    pub base: String,
    pub why: Why,
    pub fetched: bool,
}

/// Whether a PR found for the branch is its upstream's rather than its own.
/// GitHub is asked for the PR of the branch this one tracks, so a branch
/// cut from another one (`upstream`, as `upstream_apart` finds it) gets that
/// one's PR. It is the branch's own when it is of a branch by this one's
/// name, or the worktree is `wtpr`'s for that very PR (`in_dir`, the number
/// its directory carries), whose branch may be named otherwise.
fn is_the_upstreams(
    pr: &Pr,
    branch: Option<&str>,
    upstream: Option<&str>,
    in_dir: Option<u64>,
) -> bool {
    upstream.is_some() && branch != Some(pr.head.as_str()) && in_dir != Some(pr.number)
}

/// The PR GitHub has for the branch, as the workspace knows it: its base
/// branch, its number and the branch it is of there. `None` with no PR,
/// GitHub off or out of reach, or no workspace to ask.
fn pr_base(dir: &Path) -> Option<Pr> {
    crate::host::host().pr(dir).filter(|pr| safe_ref(&pr.base))
}

/// The branch's upstream when it is another branch's: neither the default
/// branch nor the branch's own copy on a remote, which a push with `-u`
/// makes its upstream. A branch cut from a feature branch has that one.
fn upstream_apart(dir: &Path, branch: &str, default: &str) -> Option<String> {
    let (upstream, remote) = upstream_of(dir, branch);
    if !safe_ref(&upstream) || !verifies(dir, &upstream) {
        return None;
    }
    // A local branch's remote is "." (or nothing), its name the whole.
    let name = upstream
        .strip_prefix(remote.as_str())
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(&upstream);
    (name != branch && name != default).then(|| upstream.clone())
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
    let branch = current_branch(dir);
    let mut default = git_as_is(dir, &["rev-parse", "--abbrev-ref", "origin/HEAD"])
        .ok()
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if let Some(rest) = default.strip_prefix("origin/") {
        default = rest.to_string();
    }
    if default.is_empty() || !safe_ref(&default) {
        default = default_base(dir).into();
    }
    let upstream = branch
        .as_deref()
        .and_then(|b| upstream_apart(dir, b, &default));
    let in_dir = crate::host::host().reviewed_pr(&facts.top);
    let pr = pr_base(dir)
        .filter(|pr| !is_the_upstreams(pr, branch.as_deref(), upstream.as_deref(), in_dir));
    // With no PR of its own, a branch cut from another branch compares with
    // that one: on origin it is fetched as any base is, elsewhere taken as
    // it is.
    let mut as_it_is = None;
    let base = match (&pr, upstream) {
        (Some(pr), _) => pr
            .base
            .strip_prefix("origin/")
            .unwrap_or(&pr.base)
            .to_string(),
        (None, Some(upstream)) => match upstream.strip_prefix("origin/") {
            Some(rest) => rest.to_string(),
            None => {
                as_it_is = Some(upstream);
                default
            }
        },
        (None, None) => default,
    };
    let (found, fetched) = match as_it_is {
        Some(upstream) => (upstream, true),
        None => {
            // A repository with no origin has nothing to fetch, its own
            // branch as fresh as it gets, so nothing is asked and nothing
            // said.
            let origin = git_as_is(dir, &["config", "--get", "remote.origin.url"]).is_ok();
            let fetched = !origin || {
                crate::host::host().count_fetch(&facts.top, "origin", "M-d diff: fetch base");
                git_as_is(dir, &["fetch", "--quiet", "origin", &base]).is_ok()
            };
            // A repository with no origin compares with its own branch.
            let found = format!("origin/{base}");
            if verifies(dir, &found) {
                (found, fetched)
            } else {
                (base, fetched)
            }
        }
    };
    let is_own_copy_on_origin = branch
        .as_deref()
        .is_some_and(|b| found.strip_prefix("origin/") == Some(b));
    let why = match pr {
        Some(pr) => Why::Pr(pr.number),
        None if is_own_copy_on_origin => Why::Own,
        None => Why::NoPr,
    };
    // Only a comparison with a freshly fetched base is worth keeping.
    if fetched {
        let _ = util::write_atomic(
            base_cache(&facts.git_dir),
            format!(
                "{}\t{found}\t{}\t{}\n",
                facts.head,
                why.word(),
                tracking_of(dir, branch.as_deref())
            ),
        );
    }
    Some(Base {
        base: found,
        why,
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
/// `staged` and its changes not staged in `diff`, and no commits. One
/// running from the merge base to the working tree has the files on disk
/// for its new side, HEAD as `right`, and what isn't committed in
/// `uncommitted`. `why` is what the base is to the branch, where M-d
/// worked the base out itself.
#[derive(Clone, Debug, Default)]
pub struct Change {
    pub top: PathBuf,
    pub base: String,
    pub why: Why,
    pub left: String,
    pub right: String,
    pub entries: Vec<Entry>,
    pub diff: String,
    pub staged: Option<String>,
    pub raw_commits: String,
    pub uncommitted: Option<Uncommitted>,
}

/// What a change running to the working tree knows of what isn't committed
/// yet: each file with changes not yet committed (`dirty`: staged, unstaged
/// or untracked, and the path a renamed one came from), and the committed
/// part, from the merge base to HEAD, read as `read_change` reads it, whose
/// story the diff tells, so it is the one told ahead, under the same key.
#[derive(Clone, Debug, Default)]
pub struct Uncommitted {
    pub dirty: BTreeSet<String>,
    pub committed: Box<Change>,
}

impl Change {
    /// Whether it is a working tree's.
    pub fn working(&self) -> bool {
        self.staged.is_some()
    }

    /// Whether it runs from the merge base to the working tree (M-d with
    /// the diff_uncommitted setting on).
    pub fn to_work_tree(&self) -> bool {
        self.uncommitted.is_some()
    }

    /// Whether `path` has changes not yet committed, in a change running to
    /// the working tree.
    pub fn dirty(&self, path: &str) -> bool {
        self.uncommitted
            .as_ref()
            .is_some_and(|u| u.dirty.contains(path))
    }

    /// The part whose story is told: the committed part of a change running
    /// to the working tree, so editing tells no new story; else the whole.
    pub fn told_part(&self) -> &Change {
        self.uncommitted.as_ref().map_or(self, |u| &u.committed)
    }

    /// The sides Haiku's order of its files is kept under
    /// (`order::order_file`): the commits compared, the working tree's side
    /// "" in a change running to it, as diffview.lua's `order_path` named a
    /// side with no commit.
    pub fn order_sides(&self) -> (&str, &str) {
        if self.to_work_tree() {
            (&self.left, "")
        } else {
            (&self.left, &self.right)
        }
    }

    /// What the diff's header says it compares with: the base and what it
    /// is to the branch, a PR's base by the name GitHub has for it.
    pub fn against(&self) -> String {
        match &self.why {
            Why::Named => format!("against {}", self.base),
            Why::Pr(number) => format!(
                "against {} (PR #{number})",
                self.base.strip_prefix("origin/").unwrap_or(&self.base)
            ),
            Why::Own => match self.unpushed() {
                0 => format!("against {} · nothing unpushed", self.base),
                n => format!("against {} · {n} unpushed", self.base),
            },
            Why::NoPr => format!("against {} (no PR)", self.base),
        }
    }

    /// How many commits the change holds, the committed part's in one
    /// running to the working tree: on the default branch, those not yet
    /// pushed.
    fn unpushed(&self) -> usize {
        self.told_part().raw_commits.matches('\x1e').count()
    }

    /// What a box says when M-d has nothing to show, as pane-in-place.sh
    /// said it rather than open an empty diff: no change past the base (and,
    /// running to the working tree, nothing uncommitted either), which on
    /// the default branch is nothing its copy on origin lacks. `None` when
    /// there is something, and always for a working tree's changes.
    pub fn nothing_to_show(&self) -> Option<String> {
        if self.working() {
            return None;
        }
        let (start, commits) = match (&self.why, self.base.split_once('/')) {
            // Commits that undo each other are unpushed all the same.
            (Why::Own, Some((remote, branch))) => (
                format!("Nothing here that {remote}'s {branch} lacks"),
                match self.unpushed() {
                    0 => "no unpushed commits".to_string(),
                    1 => "1 unpushed commit that changes no file".to_string(),
                    n => format!("{n} unpushed commits that change no file"),
                },
            ),
            _ => (
                "Nothing to show".to_string(),
                format!("no commits past {}", self.base),
            ),
        };
        match &self.uncommitted {
            Some(u) if u.committed.entries.is_empty() && u.dirty.is_empty() => {
                Some(format!("{start}: {commits} and no uncommitted changes"))
            }
            None if self.entries.is_empty() => Some(format!("{start}: {commits}")),
            _ => None,
        }
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
    /// compared (the merge base and the file on disk, for a change running
    /// to the working tree), else HEAD and the index for a staged file, the
    /// index and the working tree for one not staged.
    pub fn sides(&self, entry: &Entry) -> (String, String) {
        match entry.section {
            Section::Committed if self.to_work_tree() => (self.left.clone(), WORK_TREE.into()),
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

/// `PLAIN` for the repository at `top`, with `--text` in a worktree of
/// someone else's PR (src/trust.rs), whose `.gitattributes` could mark a
/// file binary to hide its lines, as `pr-risk` reads it.
fn plain(top: &Path) -> Vec<&'static str> {
    let mut args = PLAIN.to_vec();
    if crate::host::host().untrusted(top) {
        args.push("--text");
    }
    args
}

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
    let right = git_as_is(top, &["rev-parse", "HEAD"])?.trim().to_string();
    let left = git_as_is(top, &["merge-base", base, "HEAD"])
        .map_err(|_| anyhow!("{base} and this branch have no commit in common"))?
        .trim()
        .to_string();
    read_range(top, base, left, right)
}

/// The commits between a branch's merge base and its head, oldest first,
/// each its hash and subject, as diffview's history listed them with
/// `--reverse`.
pub fn commits_between(top: &Path, left: &str, right: &str) -> Vec<(String, String)> {
    git_as_is(
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
    let right = git_as_is(
        top,
        &["rev-parse", "--verify", &format!("{commit}^{{commit}}")],
    )
    .map_err(|_| anyhow!("No commit {commit} here"))?
    .trim()
    .to_string();
    let left = match git_as_is(
        top,
        &["rev-parse", "--verify", "--quiet", &format!("{right}^")],
    ) {
        Ok(parent) => parent.trim().to_string(),
        Err(_) => git_as_is(top, &["hash-object", "-t", "tree", "/dev/null"])?
            .trim()
            .to_string(),
    };
    read_range(top, &right, left, right.clone())
}

/// The change from `left` to `right`, what M-d and a commit of the history
/// both show.
fn read_range(top: &Path, base: &str, left: String, right: String) -> Result<Change> {
    let range = |extra: &[&str]| -> Result<String> {
        let mut args: Vec<&str> = plain(top);
        args.extend_from_slice(extra);
        args.extend_from_slice(&[left.as_str(), right.as_str()]);
        git_as_is(top, &args)
    };
    let name_status = range(&["-M", "-z", "--name-status"])?;
    let numstat = range(&["-M", "-z", "--numstat"])?;
    let entries = parse_entries(&name_status, &numstat);
    let diff = range(&["--src-prefix=a/", "--dst-prefix=b/", "-U2", "-M"])?;
    let raw_commits = git_as_is(
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
        why: Why::default(),
        left,
        right,
        entries,
        diff,
        staged: None,
        raw_commits,
        uncommitted: None,
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
    let head = git_as_is(top, &["rev-parse", "HEAD"])
        .map_err(|_| anyhow!("This repository has no commit yet"))?
        .trim()
        .to_string();
    let run = |extra: &[&str]| -> Result<String> {
        let mut args: Vec<&str> = plain(top);
        args.extend_from_slice(extra);
        git_as_is(top, &args)
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
    let unstaged = texts.last_mut().expect("two runs");
    add_untracked(top, &mut entries, unstaged, Section::Unstaged)?;
    let diff = texts.pop().unwrap_or_default();
    Ok(Change {
        top: top.to_path_buf(),
        base: String::new(),
        why: Why::default(),
        left: head.clone(),
        right: head,
        entries,
        diff,
        staged: texts.pop(),
        raw_commits: String::new(),
        uncommitted: None,
    })
}

/// Files git doesn't know yet, which its diff leaves out: each listed in
/// `section` as added, its first lines added to `diff` as an addition; one
/// the list has already is left as it is.
fn add_untracked(
    top: &Path,
    entries: &mut Vec<Entry>,
    diff: &mut String,
    section: Section,
) -> Result<()> {
    let untracked = git_as_is(top, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    for path in untracked.split('\0').filter(|p| !p.is_empty()) {
        if entries
            .iter()
            .any(|e| e.path == path && e.section == section)
        {
            continue;
        }
        let Some(text) = text_at(top, WORK_TREE, path) else {
            entries.push(Entry {
                path: path.to_string(),
                old_path: None,
                status: 'A',
                additions: 0,
                deletions: 0,
                binary: true,
                section,
            });
            continue;
        };
        let lines: Vec<&str> = text.lines().collect();
        let shown = &lines[..lines.len().min(NEW_FILE_LINES)];
        diff.push_str(&format!(
            "diff --git a/{path} b/{path}\nnew file\n@@ -0,0 +1,{} @@\n",
            shown.len()
        ));
        for line in shown {
            diff.push('+');
            diff.push_str(line);
            diff.push('\n');
        }
        entries.push(Entry {
            path: path.to_string(),
            old_path: None,
            status: 'A',
            additions: lines.len(),
            deletions: 0,
            binary: false,
            section,
        });
    }
    Ok(())
}

/// The change from `base`'s merge base with HEAD to the working tree, clean
/// or not, as diffview.lua's `uncommitted_mode` showed `:DiffviewOpen <merge
/// base>`: the branch's commits and its changes not yet committed together,
/// files git doesn't know yet among them as added (diffview listed those
/// only against the index, and M-d's diff lists them here too), with which
/// files have changes not yet committed and the committed part beside it
/// (`Uncommitted`).
pub fn read_to_work_tree(top: &Path, base: &str) -> Result<Change> {
    let committed = read_change(top, base)?;
    let left = committed.left.clone();
    let run = |extra: &[&str]| -> Result<String> {
        let mut args: Vec<&str> = plain(top);
        args.extend_from_slice(extra);
        args.push(left.as_str());
        git_as_is(top, &args)
    };
    let mut entries = parse_entries(
        &run(&["-M", "-z", "--name-status"])?,
        &run(&["-M", "-z", "--numstat"])?,
    );
    let mut diff = run(&["--src-prefix=a/", "--dst-prefix=b/", "-U2", "-M"])?;
    add_untracked(top, &mut entries, &mut diff, Section::Committed)?;
    // Read without the index's lock, which git status otherwise takes to
    // refresh it, so a commit being made meanwhile isn't refused.
    let status = git_as_is(
        top,
        &[
            "--no-optional-locks",
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
        ],
    )?;
    Ok(Change {
        top: top.to_path_buf(),
        base: base.to_string(),
        why: Why::default(),
        left,
        right: committed.right.clone(),
        entries,
        diff,
        staged: None,
        raw_commits: committed.raw_commits.clone(),
        uncommitted: Some(Uncommitted {
            dirty: parse_status(&status),
            committed: Box::new(committed),
        }),
    })
}

/// The files `git status --porcelain=v1 -z` names: each whose index or
/// working tree differs from HEAD, files git doesn't know yet among them,
/// and the path a renamed one came from, which is gone from the index, so
/// changed too (diffview.lua's `look_uncommitted`).
pub fn parse_status(status: &str) -> BTreeSet<String> {
    let mut dirty = BTreeSet::new();
    let mut fields = status.split('\0');
    while let Some(field) = fields.next() {
        let (Some(xy), Some(path)) = (field.get(..2), field.get(3..)) else {
            continue;
        };
        if path.is_empty() {
            continue;
        }
        dirty.insert(path.to_string());
        // A rename or copy names the path it came from next.
        if xy.contains(['R', 'C']) {
            let from = fields.next().unwrap_or("");
            if xy.contains('R') && !from.is_empty() {
                dirty.insert(from.to_string());
            }
        }
    }
    dirty
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
        git_bytes(top, &["show", &format!("{commit}:{path}")]).ok()?
    };
    if bytes.iter().take(8000).any(|b| *b == 0) {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// The GitHub repository `origin` names, as `owner/name`.
pub fn github_repo(top: &Path) -> Option<String> {
    let url = git_as_is(top, &["remote", "get-url", "origin"]).ok()?;
    util::github_remote(url.trim()).map(|(owner, name)| format!("{owner}/{name}"))
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
    fn status_names_each_file_not_committed_a_renamed_ones_old_path_too() {
        let status = [
            " M src/a.js",
            "A  src/new.js",
            "R  src/moved.js",
            "src/was.js",
            "C  src/copy.js",
            "src/source.js",
            "?? notes.txt",
            "",
        ]
        .join("\0");
        let dirty: Vec<String> = parse_status(&status).into_iter().collect();
        assert_eq!(
            dirty,
            [
                "notes.txt",
                "src/a.js",
                "src/copy.js",
                "src/moved.js",
                "src/new.js",
                "src/was.js"
            ],
            "a copy's source is still there, unchanged"
        );
        assert!(parse_status("").is_empty());
    }

    #[test]
    fn a_change_to_the_working_tree_has_its_commits_and_what_isnt_committed() {
        let dir = std::env::temp_dir().join(format!("sb-diff-work-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(["-c", "user.name=T", "-c", "user.email=t@example.com"])
                .args(["-c", "commit.gpgsign=false"])
                .args(args)
                .current_dir(&dir)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        run(&["init", "-q"]);
        run(&["checkout", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "a\n").unwrap();
        std::fs::write(dir.join("b.txt"), "b\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "first"]);
        run(&["checkout", "-q", "-b", "feature"]);
        let clean = read_to_work_tree(&dir, "main").unwrap();
        assert!(clean.entries.is_empty());
        assert_eq!(
            clean.nothing_to_show().as_deref(),
            Some("Nothing to show: no commits past main and no uncommitted changes")
        );
        let commits = read_change(&dir, "main").unwrap();
        assert_eq!(
            commits.nothing_to_show().as_deref(),
            Some("Nothing to show: no commits past main")
        );
        std::fs::write(dir.join("a.txt"), "A\n").unwrap();
        run(&["commit", "-q", "-am", "second"]);
        std::fs::write(dir.join("b.txt"), "B\n").unwrap();
        std::fs::write(dir.join("new.txt"), "one\ntwo\n").unwrap();
        let change = read_to_work_tree(&dir, "main").unwrap();
        assert_eq!(change.nothing_to_show(), None);
        let mut paths: Vec<&str> = change.entries.iter().map(|e| e.path.as_str()).collect();
        paths.sort();
        assert_eq!(paths, ["a.txt", "b.txt", "new.txt"]);
        let new = change.entries.iter().find(|e| e.path == "new.txt").unwrap();
        assert_eq!(
            (new.status, new.additions, new.section),
            ('A', 2, Section::Committed),
            "a file git doesn't know yet, as added"
        );
        assert!(change.diff.contains("+two\n"), "{}", change.diff);
        assert!(change.dirty("b.txt") && change.dirty("new.txt"));
        assert!(!change.dirty("a.txt"), "changed only in a commit");
        let committed = change.told_part();
        assert_eq!(committed.entries.len(), 1, "the commits' part alone");
        assert_eq!(
            crate::story::key_of(committed).0,
            crate::story::key_of(&read_change(&dir, "main").unwrap()).0,
            "its story the one told ahead"
        );
        assert_eq!(
            change.sides(new),
            (change.left.clone(), WORK_TREE.to_string())
        );
        assert_eq!(change.order_sides(), (change.left.as_str(), ""));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_header_names_the_base_and_what_it_is_to_the_branch() {
        let commit = "h\0subject\0body\x1e\n";
        let change = |base: &str, why: Why, commits: usize| Change {
            base: base.into(),
            why,
            raw_commits: commit.repeat(commits),
            entries: Vec::new(),
            ..Change::default()
        };
        assert_eq!(
            change("origin/main", Why::NoPr, 1).against(),
            "against origin/main (no PR)"
        );
        assert_eq!(
            change("origin/main", Why::Own, 3).against(),
            "against origin/main · 3 unpushed"
        );
        assert_eq!(
            change("origin/main", Why::Own, 0).against(),
            "against origin/main · nothing unpushed"
        );
        assert_eq!(
            change("origin/main", Why::Pr(123), 2).against(),
            "against main (PR #123)"
        );
        assert_eq!(
            change("origin/feature-a", Why::NoPr, 2).against(),
            "against origin/feature-a (no PR)"
        );
        assert_eq!(
            change("abc1234", Why::Named, 1).against(),
            "against abc1234"
        );
        // The unpushed commits are the committed part's, in a change running
        // to the working tree.
        let whole = Change {
            uncommitted: Some(Uncommitted {
                dirty: BTreeSet::new(),
                committed: Box::new(change("origin/main", Why::Named, 2)),
            }),
            ..change("origin/main", Why::Own, 0)
        };
        assert_eq!(whole.against(), "against origin/main · 2 unpushed");
        // With nothing unpushed, the default branch says what it lacks
        // nothing of.
        assert_eq!(
            whole_of(change("origin/main", Why::Own, 0))
                .nothing_to_show()
                .as_deref(),
            Some(
                "Nothing here that origin's main lacks: no unpushed commits and no uncommitted changes"
            )
        );
        assert_eq!(
            change("origin/main", Why::Own, 0)
                .nothing_to_show()
                .as_deref(),
            Some("Nothing here that origin's main lacks: no unpushed commits")
        );
        assert_eq!(
            change("origin/main", Why::Own, 2)
                .nothing_to_show()
                .as_deref(),
            Some("Nothing here that origin's main lacks: 2 unpushed commits that change no file"),
            "a commit and its revert"
        );
        for why in [Why::Named, Why::Pr(7), Why::Own, Why::NoPr] {
            assert_eq!(Why::from_word(&why.word()), Some(why));
        }
        assert_eq!(Why::from_word("pr:x"), None);
    }

    /// `change` as one running to a clean working tree, with no commits.
    fn whole_of(change: Change) -> Change {
        Change {
            uncommitted: Some(Uncommitted {
                dirty: BTreeSet::new(),
                committed: Box::new(Change::default()),
            }),
            ..change
        }
    }

    #[test]
    fn a_base_kept_with_no_pr_is_asked_for_again_after_ten_minutes() {
        let secs = std::time::Duration::from_secs;
        assert!(!pr_may_have_come(&Why::NoPr, secs(60), true));
        assert!(pr_may_have_come(&Why::NoPr, secs(11 * 60), true));
        assert!(
            !pr_may_have_come(&Why::NoPr, secs(11 * 60), false),
            "GitHub off"
        );
        for why in [Why::Pr(3), Why::Own, Why::Named] {
            assert!(!pr_may_have_come(&why, secs(3600), true), "{why:?}");
        }
    }

    #[test]
    fn a_pr_found_through_the_branchs_upstream_is_the_upstreams() {
        let pr = Pr {
            base: "main".into(),
            number: 12,
            head: "under".into(),
        };
        let upstream = Some("origin/under");
        assert!(
            is_the_upstreams(&pr, Some("over"), upstream, None),
            "over was cut from under, whose PR it is"
        );
        assert!(!is_the_upstreams(&pr, Some("under"), None, None));
        assert!(
            !is_the_upstreams(&pr, Some("pr-review/12"), upstream, Some(12)),
            "wtpr's worktree for that PR, its branch named otherwise"
        );
        assert!(is_the_upstreams(&pr, Some("over"), upstream, Some(7)));
        assert!(
            !is_the_upstreams(&pr, Some("over"), None, None),
            "no upstream apart"
        );
    }

    #[test]
    fn a_branch_cut_from_another_branch_compares_with_that_one() {
        let dir = std::env::temp_dir().join(format!("sb-diff-upstream-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(["-c", "user.name=T", "-c", "user.email=t@example.com"])
                .args(["-c", "commit.gpgsign=false"])
                .args(args)
                .current_dir(&dir)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        run(&["init", "-q"]);
        run(&["checkout", "-q", "-b", "main"]);
        run(&["commit", "-q", "--allow-empty", "-m", "first"]);
        run(&["branch", "-q", "feature-a"]);
        // Origin's copies, as a fetch leaves them.
        run(&["config", "remote.origin.url", "/nowhere"]);
        run(&[
            "config",
            "remote.origin.fetch",
            "+refs/heads/*:refs/remotes/origin/*",
        ]);
        for branch in ["main", "feature-a", "child"] {
            run(&[
                "update-ref",
                &format!("refs/remotes/origin/{branch}"),
                "HEAD",
            ]);
        }
        run(&["checkout", "-q", "-b", "child", "--no-track"]);
        assert_eq!(upstream_apart(&dir, "child", "main"), None, "no upstream");
        run(&["branch", "-q", "--set-upstream-to=feature-a", "child"]);
        assert_eq!(
            upstream_apart(&dir, "child", "main").as_deref(),
            Some("feature-a"),
            "a local branch"
        );
        run(&[
            "branch",
            "-q",
            "--set-upstream-to=origin/feature-a",
            "child",
        ]);
        assert_eq!(
            upstream_apart(&dir, "child", "main").as_deref(),
            Some("origin/feature-a")
        );
        run(&["branch", "-q", "--set-upstream-to=origin/child", "child"]);
        assert_eq!(
            upstream_apart(&dir, "child", "main"),
            None,
            "its own copy on origin, as a push with -u leaves it"
        );
        for default in ["main", "origin/main"] {
            run(&[
                "branch",
                "-q",
                &format!("--set-upstream-to={default}"),
                "child",
            ]);
            assert_eq!(upstream_apart(&dir, "child", "main"), None, "{default}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_a_branch_names_characters_go_on_a_command_line() {
        assert!(safe_ref("origin/feature-x_1.2"));
        assert!(!safe_ref("main; rm -rf ~"));
        assert!(!safe_ref(""));
    }
}
