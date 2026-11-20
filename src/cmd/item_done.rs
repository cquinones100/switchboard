//! `switchboard item-done --says '<what it does now>' [--hash <commit>]
//! [--already-on <branch>] [--merged-pr-head <commit>] [--checked-in
//! <branch>] [--without-asking] [--close-issue] [<worktree>]`: the end of a
//! landing, as queue-and-clean's "Closing out" and clean-worktree's steps 2
//! to 5 had Claude do it by hand. Run from the worktree (or given its
//! path) once its work is on the branch it lands on, it
//!
//! 1. checks nothing would be lost: no uncommitted changes, and every
//!    commit of the branch on a branch of the remote as it is now (fetched
//!    with its deleted branches pruned, so a branch deleted there without
//!    landing doesn't count) or its work
//!    in the branch it lands on (`--already-on`'s, else its `wtnTarget`,
//!    else the default branch, its remote fetched and pruned too), a squash
//!    included, as queue-and-clean's "No PR" judges it, or HEAD being the
//!    head of the PR GitHub merged (`--merged-pr-head`, its `headRefOid`
//!    read just before), which covers a squash merge whose branch GitHub
//!    deleted and whose files the default branch has changed since, or
//!    `--checked-in <branch>`, every change found there by reading it, as
//!    queue-and-clean's "No PR" does when the default branch has changed the
//!    same files since a squash;
//! 2. ticks the item the branch was started for, which `wtn` keeps as
//!    `branch.<branch>.wtnItem` (`<note's ID>: <item's first line>`), in
//!    the note found by its ID wherever it is now, with an indented
//!    `Done: <hash>, <took>[, landed without asking]. <says>` line under it
//!    (`Done: <hash>, already on <branch>. <says>` for an item found done),
//!    moved under the unticked items as Notes moves one
//!    (`format::toggle_check`), under the note's ID lock, so the app takes it
//!    in as any change made from outside; a branch `wtn` didn't start has
//!    no item, nor one started from a GitHub issue, and one already ticked
//!    is left as it is;
//! 3. with `--close-issue` (passed once the landing's question named it, or
//!    for a landing made without asking), closes the GitHub issue `wtn
//!    --issue` started the branch from (its `branch.<branch>.wtnIssue`;
//!    never one a branch's name alone suggests, which may be another
//!    tracker's number), whoever's tab it was, autopilot's or yours: once the
//!    commit is on the default branch on GitHub, a comment reading as the
//!    `Done:` line above would, naming the claim it releases, and the issue
//!    closed as done (`issues::close_landed`), or only the comment when
//!    GitHub closed it as done by a PR's `Closes #<n>`; the note is never
//!    ticked for it, issues and notes being two sources. Its failure (GitHub
//!    off in Settings, the commit not there yet) is said and leaves the rest
//!    to go on, since the work is safe either way;
//! 4. removes the worktree from the main clone, deletes the branch (`-D`,
//!    since a squash leaves it looking unmerged).
//!
//! The tick comes before the removal, so an item is never unticked with
//! its worktree gone, which autopilot and `wtn` would take as free. It
//! stops at the first step that fails, saying why, and leaves the rest:
//! nothing is removed when the item can't be ticked. Moving the tab back
//! to the main clone stays queue-and-clean's last step (`switchboard
//! respawn`), since that ends the session, which writes its final line
//! from what this prints first.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::autopilot::issues::{self, Closed};
use crate::cmd::landing;
use crate::cmd::worktree_from_note::{notes_dir, tracked};
use crate::integrations::github::Repo;
use crate::notes::format::{Doc, toggle_check};
use crate::notes::render;
use crate::notes::text::Text;
use crate::notes::{format, id};
use crate::{util, worktree};

const USAGE: &str = "Usage: switchboard item-done --says '<what it does now>' [--hash <commit>] [--already-on <branch>] [--merged-pr-head <commit>] [--checked-in <branch>] [--without-asking] [--close-issue] [<worktree>]\n\n\
     Ticks the item the worktree's branch was started for, with its Done line,\n\
     or closes the GitHub issue it was started from with that line as its comment,\n\
     then removes the worktree and its branch once nothing in them would be lost.\n\n  \
     --says            what the item does now, in prose, ending the Done line\n  \
     --hash            the commit that did it (the worktree's HEAD by default)\n  \
     --already-on      the item was found done on that branch\n  \
     --merged-pr-head  the merged PR's head commit, as GitHub gives it\n  \
     --checked-in      every change was found in that branch by reading it\n  \
     --without-asking  it landed without asking (autopilot's Land without asking)\n  \
     --close-issue     close the GitHub issue the branch was started from, as asked\n";

/// What `--says` and the rest gave.
struct Given {
    says: String,
    hash: Option<String>,
    already_on: Option<String>,
    merged_pr_head: Option<String>,
    checked_in: Option<String>,
    without_asking: bool,
    close_issue: bool,
    dir: PathBuf,
}

fn given(args: &[String]) -> Result<Given, String> {
    let parsed = worktree::Given::parse(
        args,
        &[
            "--says",
            "--hash",
            "--already-on",
            "--merged-pr-head",
            "--checked-in",
        ],
    )
    .map_err(|flag| format!("{flag} needs a value"))?;
    let mut without_asking = false;
    let mut close_issue = false;
    let mut dir = None;
    for a in &parsed.positional {
        match a.as_str() {
            "--without-asking" => without_asking = true,
            "--close-issue" => close_issue = true,
            other if other.starts_with('-') => return Err(format!("unknown option {other}")),
            other if dir.is_none() => dir = Some(PathBuf::from(other)),
            other => return Err(format!("one worktree only, not {other} too")),
        }
    }
    let says = parsed
        .last("--says")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("--says is needed: what the item does now")?
        .to_string();
    Ok(Given {
        says,
        hash: parsed.last("--hash").map(str::to_string),
        already_on: parsed.last("--already-on").map(str::to_string),
        merged_pr_head: parsed.last("--merged-pr-head").map(str::to_string),
        checked_in: parsed.last("--checked-in").map(str::to_string),
        without_asking,
        close_issue,
        dir: dir.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into())),
    })
}

pub fn main(args: Vec<String>) -> ExitCode {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        eprint!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let given = match given(&args) {
        Ok(g) => g,
        Err(e) => {
            eprint!("item-done: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(&given, &notes_dir(), &close_on_github) {
        Ok(lines) => {
            for line in lines {
                println!("{line}");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("item-done: {e}");
            ExitCode::from(1)
        }
    }
}

/// Step 3 on GitHub: `issues::close_landed`, run to its end.
fn close_on_github(repo: &Repo, number: u64, hash: &str, done: &str) -> Result<Closed, String> {
    issues::wait_for(issues::close_landed(repo, number, hash, done)).map_err(|e| format!("{e:#}"))
}

/// What closes an issue, `close_on_github` but in tests.
type Close<'a> = &'a dyn Fn(&Repo, u64, &str, &str) -> Result<Closed, String>;

/// The steps, as the module's opening says; the lines to print.
fn run(given: &Given, notes: &Path, close: Close) -> Result<Vec<String>, String> {
    let git = |dir: &Path, args: &[&str]| {
        worktree::git_at(dir, args).map_err(|e| format!("git {}: {}", args[0], first_line(&e)))
    };
    let top = PathBuf::from(git(&given.dir, &["rev-parse", "--show-toplevel"])?);
    let main = crate::mux::main_clone::of(&top)
        .ok_or_else(|| format!("{} isn't in a clone with a main worktree", top.display()))?;
    if same(&main, &top) {
        return Err(format!(
            "{} is the main clone, not a worktree",
            top.display()
        ));
    }
    // Someone else's PR checked out to review (`wtpr --untrusted`) never
    // lands from here, and the merge below could run a merge driver its
    // files pick.
    if crate::trust::is_untrusted(&top) {
        return Err(format!(
            "{} holds someone else's PR checked out for review, which item-done leaves alone",
            top.display()
        ));
    }
    let branch = git(&top, &["branch", "--show-current"])?;
    if branch.is_empty() {
        return Err("the worktree has no branch checked out".into());
    }
    let changes = git(&top, &["status", "--porcelain"])?;
    if !changes.is_empty() {
        return Err(format!(
            "uncommitted changes in {} would be lost:\n{changes}",
            top.display()
        ));
    }
    // A remote that can't be fetched proves nothing, but a landing kept
    // locally may still show the work is safe.
    let remote = worktree::default_remote(&main);
    let (remote, unfetched) = match remote {
        Some(r) if !fetch_all(&main, &r) => (None, Some(r)),
        r => (r, None),
    };
    let lands_on = lands_on(
        &main,
        &top,
        &branch,
        given.already_on.as_deref(),
        remote.as_deref(),
    )
    // The branch being deleted proves nothing about itself.
    .filter(|r| *r != format!("refs/heads/{branch}") && *r != branch);
    let head = git(&top, &["rev-parse", "HEAD"])?;
    // Read by hand, in a branch other than the one deleted, which is there:
    // a local branch, or one of the fetched remote's, by its name, never a
    // revision.
    let own = format!("refs/heads/{branch}");
    let checked_elsewhere = given.checked_in.as_deref().is_some_and(|c| {
        // A remote-tracking branch only of the remote just fetched and
        // pruned, so it is there now.
        let fresh = remote
            .as_deref()
            .is_some_and(|r| c.starts_with(&format!("{r}/")));
        [
            Some(format!("refs/heads/{c}")),
            fresh.then(|| format!("refs/remotes/{c}")),
        ]
        .into_iter()
        .flatten()
        .find(|r| worktree::git_ok(&top, &["show-ref", "--verify", "--quiet", r]))
        .is_some_and(|r| r != own)
    });
    let pr_merged_from_here = given.merged_pr_head.as_deref() == Some(head.as_str());
    if !pr_merged_from_here
        && !checked_elsewhere
        && !remote.as_deref().is_some_and(|r| on_remote(&top, r))
        && !lands_on.as_deref().is_some_and(|r| holds(&top, r))
    {
        return Err(format!(
            "{branch} has commits on no remote branch{}{}: removing it would lose them",
            lands_on
                .map(|r| format!(", and {r} lacks its work"))
                .unwrap_or_default(),
            unfetched
                .map(|r| format!(" (git fetch {r} failed)"))
                .unwrap_or_default()
        ));
    }
    let mut out = Vec::new();
    let claim = worktree::git_at(
        &main,
        &["config", "--get", &format!("branch.{branch}.wtnItem")],
    )
    .unwrap_or_default();
    let hash = match &given.hash {
        Some(h) => h.clone(),
        None => git(&top, &["rev-parse", "--short", "HEAD"])?,
    };
    let took = landing::started_at(&top).map(|s| crate::sidebar::text::span(util::now_secs() - s));
    let done = done_line(
        &hash,
        took.as_deref(),
        given.already_on.as_deref(),
        given.without_asking,
        &given.says,
    );
    let issue = issue_of(&main, &branch);
    match note_and_item(&claim, notes) {
        None => out.push(match &issue {
            Some((repo, number)) => format!(
                "no item: {branch} was started from {repo}#{number}, a GitHub issue rather than a note's item"
            ),
            None => format!("no item: {branch} wasn't started from a note's item"),
        }),
        Some((file, first)) => out.push(tick_file(&file?, &first, &done)?),
    }
    if let Some((repo, number)) = issue {
        out.push(close_issue(given, &top, &hash, &done, &repo, number, close));
    }
    let top_text = top.to_string_lossy().into_owned();
    git(&main, &["worktree", "remove", &top_text])
        .map_err(|e| e.replacen("git worktree:", "git worktree remove:", 1))?;
    out.push(format!("removed: {top_text}"));
    git(&main, &["branch", "-D", &branch])
        .map_err(|e| e.replacen("git branch:", "git branch -D:", 1))?;
    out.push(format!("deleted: {branch}"));
    out.push(format!("main clone: {}", main.display()));
    Ok(out)
}

/// Step 3: the issue closed with the `Done:` line `done` as its comment, when
/// asked to (`--close-issue`) and leak-check's scan finds nothing in it,
/// since it is public; the line saying what became of it.
fn close_issue(
    given: &Given,
    top: &Path,
    hash: &str,
    done: &str,
    repo: &Repo,
    number: u64,
    close: Close,
) -> String {
    if !given.close_issue {
        return format!(
            "left open: {repo}#{number}: --close-issue not given, so not asked to close it"
        );
    }
    use crate::cmd::leak_check;
    let hits = match leak_check::read_words(&leak_check::words_file()) {
        Ok(words) => leak_check::hits_in(done, false, &words),
        Err(e) => return format!("left open: {repo}#{number}: leak-check couldn't look: {e}"),
    };
    if !hits.is_empty() {
        let found: Vec<String> = hits
            .iter()
            .map(|(kind, shown)| format!("{kind} \"{shown}\""))
            .collect();
        return format!(
            "left open: {repo}#{number}: leak-check found {} in its comment",
            found.join(", ")
        );
    }
    // Whole, for GitHub's check.
    let whole = worktree::git_at(
        top,
        &["rev-parse", "--verify", &format!("{hash}^{{commit}}")],
    )
    .unwrap_or_else(|_| hash.to_string());
    match close(repo, number, &whole, done) {
        Ok(Closed::Closed) => format!("closed: {repo}#{number}"),
        Ok(Closed::Commented) => {
            format!("commented: {repo}#{number}, which GitHub had closed as done")
        }
        Ok(Closed::LeftAlone(how)) => format!("left: {repo}#{number}, closed already ({how})"),
        Err(e) => format!("left open: {repo}#{number}: {e}"),
    }
}

/// The GitHub issue `wtn --issue` started `branch` from, its `wtnIssue`.
fn issue_of(main: &Path, branch: &str) -> Option<(Repo, u64)> {
    issues::parse_reference(&crate::cmd::worktree_from_issue::issue_of_branch(
        main, branch,
    )?)
}

/// Fetches every branch of `remote` into its remote-tracking refs, those
/// of branches deleted there pruned, whatever the remote's configured
/// refspec covers, so no stale ref of it is taken for a live branch.
fn fetch_all(main: &Path, remote: &str) -> bool {
    worktree::count_fetch(main, remote, "item-done");
    worktree::git_loud(
        main,
        &[
            "fetch",
            "-q",
            "--prune",
            remote,
            &format!("+refs/heads/*:refs/remotes/{remote}/*"),
        ],
        true,
    )
}

/// An error's first line, as git's own message.
fn first_line(e: &anyhow::Error) -> String {
    e.to_string().lines().next().unwrap_or_default().to_string()
}

fn same(a: &Path, b: &Path) -> bool {
    let real = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    real(a) == real(b)
}

/// The ref of the branch the work lands on: `--already-on`'s, else the
/// target `wtn` cut the branch from, else the default branch, each as its
/// remote branch when it tracks one (fetched and pruned first when it isn't
/// `fetched`, the remote already fetched) and the local branch otherwise.
fn lands_on(
    main: &Path,
    top: &Path,
    branch: &str,
    already_on: Option<&str>,
    fetched: Option<&str>,
) -> Option<String> {
    let name = already_on.map(str::to_string).or_else(|| {
        worktree::git_at(
            main,
            &["config", "--get", &format!("branch.{branch}.wtnTarget")],
        )
        .ok()
        .filter(|t| !t.is_empty())
    });
    let exists = |r: &str| worktree::git_ok(top, &["rev-parse", "--verify", "--quiet", r]);
    let name = match name {
        Some(n) => n,
        None => {
            let common = worktree::git_at(
                main,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            )
            .ok()?;
            let name = crate::tabs::git::default_branch(Path::new(&common))?;
            let fresh = fetched.map(|r| format!("refs/remotes/{r}/{name}"));
            return Some(
                fresh
                    .filter(|r| exists(r))
                    .unwrap_or_else(|| format!("refs/heads/{name}")),
            );
        }
    };
    if let Some((remote, remote_branch)) = tracked(main, &name) {
        if fetched != Some(remote.as_str()) && !fetch_all(main, &remote) {
            return None;
        }
        let r = format!("refs/remotes/{remote}/{remote_branch}");
        if exists(&r) {
            return Some(r);
        }
    }
    // A branch that tracks none lands locally (queue-and-clean's target
    // with no remote), so its own ref is what holds the work.
    Some(format!("refs/heads/{name}"))
}

/// Whether a branch of `remote`, just fetched and pruned, holds the
/// worktree's HEAD; another remote's branches, never fetched here, may be
/// stale and don't count.
fn on_remote(top: &Path, remote: &str) -> bool {
    worktree::git_at(
        top,
        &[
            "branch",
            "-r",
            "--contains",
            "HEAD",
            "--list",
            &format!("{remote}/*"),
        ],
    )
    .is_ok_and(|names| !names.trim().is_empty())
}

/// Whether `target` holds the work of the worktree's HEAD: it contains the
/// commit, or every file the branch changed from where it left the target
/// reads there as it does at HEAD (a squash, a rebase). Compared file by
/// file, never merged, so no merge driver or textconv of the repository's
/// can run or decide it.
fn holds(top: &Path, target: &str) -> bool {
    if worktree::git_ok(top, &["merge-base", "--is-ancestor", "HEAD", target]) {
        return true;
    }
    let Ok(base) = worktree::git_at(top, &["merge-base", target, "HEAD"]) else {
        return false;
    };
    let plain = [
        "--no-ext-diff",
        "--no-textconv",
        "--no-renames",
        "--ignore-submodules=none",
    ];
    let mut changed = vec!["diff", "--name-only", "-z"];
    changed.extend(plain);
    changed.extend([base.as_str(), "HEAD"]);
    // Read as git wrote them, untrimmed, so a name's own spaces stay; a
    // name that isn't UTF-8 can't be passed back exactly, so proves nothing.
    let mut list = std::process::Command::new("git");
    list.arg("-C").arg(top).args(&changed);
    let Some(names) = util::output_of(&mut list)
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
    else {
        return false;
    };
    let paths: Vec<String> = names
        .split('\0')
        .filter(|n| !n.is_empty())
        .map(|n| format!(":(literal){n}"))
        .collect();
    let mut same = vec!["diff", "--quiet"];
    same.extend(plain);
    same.extend(["HEAD", target, "--"]);
    same.extend(paths.iter().map(String::as_str));
    paths.is_empty() || worktree::git_ok(top, &same)
}

/// The note and the item a `wtnItem` claim names: by the note's ID, found
/// wherever it is now, or by its path in the notes directory for a claim
/// from before notes had IDs. `None` for no claim; the note an error when
/// it can't be found.
fn note_and_item(claim: &str, notes: &Path) -> Option<(Result<PathBuf, String>, String)> {
    if claim.trim().is_empty() {
        return None;
    }
    if let Some((note_id, first)) = id::claim_of(claim) {
        let file = id::find_by_id(note_id, notes).ok_or_else(|| {
            format!(
                "couldn't find the note with ID {note_id} in {}",
                notes.display()
            )
        });
        return Some((file, first.trim().to_string()));
    }
    let (path, first) = claim.split_once(": ")?;
    let file = notes.join(path);
    let file = if file.is_file() {
        Ok(file)
    } else {
        Err(format!(
            "couldn't find the note {path} in {}",
            notes.display()
        ))
    };
    Some((file, first.trim().to_string()))
}

/// The line under a ticked item, as the module's opening says.
pub fn done_line(
    hash: &str,
    took: Option<&str>,
    already_on: Option<&str>,
    without_asking: bool,
    says: &str,
) -> String {
    let how = match already_on {
        Some(branch) => format!(", already on {branch}"),
        None => format!(
            "{}{}",
            took.map(|t| format!(", {t}")).unwrap_or_default(),
            if without_asking {
                ", landed without asking"
            } else {
                ""
            }
        ),
    };
    format!("Done: {hash}{how}. {}", says.trim())
}

/// What ticking did to a note's text.
#[derive(Debug, PartialEq, Eq)]
pub enum Ticked {
    /// The note's text with the item ticked.
    Now(String),
    /// The item was ticked already.
    Already,
}

/// The note's text with the top-level item reading `first` ticked, `done`
/// indented under it after what is indented under it already, and moved
/// under the unticked items as Notes moves one; `None` when no item reads
/// so.
pub fn tick(text: &str, first: &str, done: &str) -> Option<Ticked> {
    let mut doc = Doc::new(Text::from_file(text));
    let lines = doc.text.lines();
    let reads = |line: &str, mark: &[&str]| {
        mark.iter().any(|m| {
            line.strip_prefix(m)
                .is_some_and(|rest| rest.trim() == first)
        })
    };
    let Some(row) = lines.iter().position(|l| reads(l, &["- [ ] "])) else {
        return lines
            .iter()
            .any(|l| reads(l, &["- [x] ", "- [X] "]))
            .then_some(Ticked::Already);
    };
    let mut last = row;
    while lines
        .get(last + 1)
        .is_some_and(|l| format::has_text(l) && render::indent_of(l) > 0)
    {
        last += 1;
    }
    doc.text
        .set_lines(last + 1, last + 1, &[format!("  {done}")]);
    toggle_check(&mut doc, row);
    Some(Ticked::Now(doc.text.file_text()))
}

/// Ticks the item in its note, under the note's ID lock, and says so.
fn tick_file(file: &Path, first: &str, done: &str) -> Result<String, String> {
    let name = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    id::locked(file, || {
        let text = std::fs::read_to_string(file)
            .map_err(|e| format!("couldn't read {}: {e}", file.display()))?;
        match tick(&text, first, done) {
            None => Err(format!("no item reading \"{first}\" in {name}")),
            Some(Ticked::Already) => Ok(format!("already ticked: \"{first}\" in {name}")),
            Some(Ticked::Now(new)) => {
                util::write_atomic(file, new)
                    .map_err(|e| format!("couldn't write {}: {e}", file.display()))?;
                Ok(format!("ticked: \"{first}\" in {name}"))
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    const NOTE: &str = "# Work\n\n- [ ] FEAT: Other\n- [ ] FEAT: Copy the reply\n  - Seen on Monday\n  Plan: [[Plan - Copy]]\n- [ ] BUG: Last\n\n- [x] FEAT: Old\n  Done: abc, 5m. Old.\n\n<!-- note-id: n-0123456789ab -->\n";

    #[test]
    fn the_item_is_ticked_with_its_done_line_and_moved_under_the_unticked_ones() {
        let Some(Ticked::Now(text)) = tick(
            NOTE,
            "FEAT: Copy the reply",
            "Done: 1234abcd, 1h2m. Copies.",
        ) else {
            panic!("not ticked");
        };
        assert_eq!(
            text,
            "# Work\n\n- [ ] FEAT: Other\n- [ ] BUG: Last\n\n- [x] FEAT: Copy the reply\n  - Seen on Monday\n  Plan: [[Plan - Copy]]\n  Done: 1234abcd, 1h2m. Copies.\n- [x] FEAT: Old\n  Done: abc, 5m. Old.\n\n<!-- note-id: n-0123456789ab -->\n"
        );
    }

    #[test]
    fn an_item_ticked_already_is_left_and_one_not_there_is_none() {
        assert_eq!(tick(NOTE, "FEAT: Old", "Done: x."), Some(Ticked::Already));
        assert_eq!(tick(NOTE, "FEAT: Missing", "Done: x."), None);
    }

    #[test]
    fn the_done_line_says_how_it_landed() {
        assert_eq!(
            done_line("1234abcd", Some("32m"), None, true, "It copies. "),
            "Done: 1234abcd, 32m, landed without asking. It copies."
        );
        assert_eq!(
            done_line("1234abcd", None, None, false, "It copies."),
            "Done: 1234abcd. It copies."
        );
        assert_eq!(
            done_line("1234abcd", Some("32m"), Some("main"), true, "It copies."),
            "Done: 1234abcd, already on main. It copies."
        );
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-C"])
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A main clone with an origin, and a worktree on `note/copy` started
    /// for the note's item, its work committed.
    fn landed_setup(name: &str) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("sb-item-done-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let origin = dir.join("origin.git");
        let main = dir.join("main");
        let notes = dir.join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        std::fs::create_dir_all(&main).unwrap();
        git(
            &dir,
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                origin.to_str().unwrap(),
            ],
        );
        git(&main, &["init", "-q", "-b", "main"]);
        git(&main, &["commit", "-q", "--allow-empty", "-m", "one"]);
        git(
            &main,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        git(&main, &["push", "-q", "-u", "origin", "main"]);
        git(&main, &["remote", "set-head", "origin", "main"]);
        let wt = main.join(".claude/worktrees/note-copy");
        git(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "note/copy",
                wt.to_str().unwrap(),
            ],
        );
        std::fs::write(wt.join("copy.txt"), "copied\n").unwrap();
        git(&wt, &["add", "copy.txt"]);
        git(&wt, &["commit", "-q", "-m", "Copy the reply"]);
        // Renamed since wtn claimed it: found by its ID.
        std::fs::write(notes.join("Work renamed.md"), NOTE).unwrap();
        git(
            &main,
            &[
                "config",
                "branch.note/copy.wtnItem",
                "n-0123456789ab: FEAT: Copy the reply",
            ],
        );
        (dir, main, wt, notes)
    }

    /// No issue is closed in a test but where one is looked for.
    fn never(_: &Repo, _: u64, _: &str, _: &str) -> Result<Closed, String> {
        panic!("no issue to close")
    }

    fn given_for(wt: &Path) -> Given {
        Given {
            says: "It copies.".into(),
            hash: Some("1234abcd".into()),
            already_on: None,
            merged_pr_head: None,
            checked_in: None,
            without_asking: false,
            close_issue: false,
            dir: wt.to_path_buf(),
        }
    }

    #[test]
    fn a_landed_worktree_is_ticked_removed_and_its_branch_deleted() {
        let (dir, main, wt, notes) = landed_setup("landed");
        git(&wt, &["push", "-q", "origin", "HEAD:main"]);
        let out = run(&given_for(&wt), &notes, &never).unwrap();
        assert!(out.iter().any(|l| l.starts_with("ticked:")), "{out:?}");
        assert!(!wt.exists(), "the worktree is removed");
        assert!(
            !worktree::branch_exists(&main, "note/copy"),
            "the branch is deleted"
        );
        let text = std::fs::read_to_string(notes.join("Work renamed.md")).unwrap();
        assert!(text.contains("- [x] FEAT: Copy the reply\n  - Seen on Monday\n  Plan: [[Plan - Copy]]\n  Done: 1234abcd. It copies.\n"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn work_on_no_remote_is_neither_ticked_nor_removed() {
        let (dir, main, wt, notes) = landed_setup("unlanded");
        let err = run(&given_for(&wt), &notes, &never).unwrap_err();
        assert!(err.contains("would lose them"), "{err}");
        assert!(wt.exists());
        assert!(worktree::branch_exists(&main, "note/copy"));
        assert_eq!(
            std::fs::read_to_string(notes.join("Work renamed.md")).unwrap(),
            NOTE
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_branch_deleted_on_the_remote_without_landing_doesnt_count() {
        let (dir, main, wt, notes) = landed_setup("deleted");
        git(&wt, &["push", "-q", "origin", "HEAD:refs/heads/note/copy"]);
        git(&main, &["fetch", "-q", "origin"]);
        let origin = dir.join("origin.git");
        git(&origin, &["branch", "-D", "note/copy"]);
        let err = run(&given_for(&wt), &notes, &never).unwrap_err();
        assert!(err.contains("would lose them"), "{err}");
        assert!(wt.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_merged_prs_head_counts_however_the_default_branch_changed_since() {
        let (dir, main, wt, notes) = landed_setup("merged-pr");
        std::fs::write(main.join("copy.txt"), "copied\nand more since\n").unwrap();
        git(&main, &["add", "copy.txt"]);
        git(
            &main,
            &["commit", "-q", "-m", "Squash-merged, then changed"],
        );
        git(&main, &["push", "-q", "origin", "main"]);
        assert!(
            run(&given_for(&wt), &notes, &never).is_err(),
            "no proof without the PR"
        );
        let mut given = given_for(&wt);
        given.merged_pr_head = Some(git(&wt, &["rev-parse", "HEAD"]));
        run(&given, &notes, &never).unwrap();
        assert!(!wt.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn work_checked_in_the_default_branch_by_reading_it_counts() {
        let (dir, main, wt, notes) = landed_setup("checked-in");
        std::fs::write(main.join("copy.txt"), "copied\nand more since\n").unwrap();
        git(&main, &["add", "copy.txt"]);
        git(&main, &["commit", "-q", "-m", "Squashed, then changed"]);
        git(&main, &["push", "-q", "origin", "main"]);
        let mut given = given_for(&wt);
        given.checked_in = Some("main".into());
        run(&given, &notes, &never).unwrap();
        assert!(!wt.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn work_landed_on_a_local_target_counts_however_far_behind_origin_is() {
        let (dir, main, wt, notes) = landed_setup("local-target");
        git(&main, &["branch", "integration", "main"]);
        git(&main, &["push", "-q", "origin", "integration"]);
        git(
            &main,
            &["config", "branch.note/copy.wtnTarget", "integration"],
        );
        git(&wt, &["push", "-q", ".", "HEAD:integration"]);
        run(&given_for(&wt), &notes, &never).unwrap();
        assert!(!wt.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_local_landing_needs_no_reachable_remote() {
        let (dir, main, wt, notes) = landed_setup("offline");
        git(
            &main,
            &["config", "branch.note/copy.wtnTarget", "integration"],
        );
        git(&main, &["branch", "integration", "main"]);
        git(&wt, &["push", "-q", ".", "HEAD:integration"]);
        git(
            &main,
            &["remote", "set-url", "origin", "/nowhere/at/all.git"],
        );
        run(&given_for(&wt), &notes, &never).unwrap();
        assert!(!wt.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn checked_in_never_takes_the_branch_being_deleted() {
        let (dir, _main, wt, notes) = landed_setup("checked-self");
        for own in ["note/copy", "HEAD", "refs/heads/note/copy"] {
            let mut given = given_for(&wt);
            given.checked_in = Some(own.into());
            assert!(run(&given, &notes, &never).is_err(), "{own}");
        }
        assert!(wt.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn work_squashed_onto_the_default_branch_counts_as_landed() {
        let (dir, main, wt, notes) = landed_setup("squashed");
        std::fs::write(main.join("copy.txt"), "copied\n").unwrap();
        git(&main, &["add", "copy.txt"]);
        git(&main, &["commit", "-q", "-m", "Copy the reply, squashed"]);
        git(&main, &["push", "-q", "origin", "main"]);
        run(&given_for(&wt), &notes, &never).unwrap();
        assert!(!wt.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_item_not_in_its_note_stops_before_anything_is_removed() {
        let (dir, _main, wt, notes) = landed_setup("missing");
        git(&wt, &["push", "-q", "origin", "HEAD:main"]);
        std::fs::write(
            notes.join("Work renamed.md"),
            NOTE.replace("FEAT: Copy the reply", "FEAT: Reworded"),
        )
        .unwrap();
        let err = run(&given_for(&wt), &notes, &never).unwrap_err();
        assert!(err.contains("no item reading"), "{err}");
        assert!(wt.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A worktree on `branch`, its work on origin's main, instead of the
    /// note's item.
    fn issue_setup(name: &str, branch: &str) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
        let (dir, main, wt, notes) = landed_setup(name);
        git(&main, &["config", "--unset", "branch.note/copy.wtnItem"]);
        git(&wt, &["checkout", "-q", "-b", branch]);
        git(&main, &["branch", "-q", "-D", "note/copy"]);
        git(&wt, &["push", "-q", "origin", "HEAD:main"]);
        (dir, main, wt, notes)
    }

    #[test]
    fn an_issue_branch_closes_its_issue_with_the_done_line_and_ticks_nothing() {
        let (dir, main, wt, notes) = issue_setup("issue", "issue/12-copy-the-reply");
        git(
            &main,
            &[
                "config",
                "branch.issue/12-copy-the-reply.wtnIssue",
                "acme/widgets#12",
            ],
        );
        let asked = std::cell::RefCell::new(Vec::new());
        let close = |repo: &Repo, n: u64, hash: &str, done: &str| {
            asked
                .borrow_mut()
                .push((repo.to_string(), n, hash.to_string(), done.to_string()));
            Ok(Closed::Closed)
        };
        let before = std::fs::read_to_string(notes.join("Work renamed.md")).unwrap();
        let given = Given {
            close_issue: true,
            ..given_for(&wt)
        };
        let out = run(&given, &notes, &close).unwrap();
        assert!(
            out.contains(&"closed: acme/widgets#12".to_string()),
            "{out:?}"
        );
        assert_eq!(
            *asked.borrow(),
            [(
                "acme/widgets".to_string(),
                12,
                "1234abcd".to_string(),
                "Done: 1234abcd. It copies.".to_string()
            )]
        );
        assert_eq!(
            std::fs::read_to_string(notes.join("Work renamed.md")).unwrap(),
            before,
            "the note is left alone"
        );
        assert!(!wt.exists(), "the worktree is removed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_issue_wtn_recorded_is_closed_and_one_that_fails_leaves_the_rest_to_go_on() {
        let (dir, main, wt, notes) = issue_setup("wtn-issue", "issue/7-x");
        git(
            &main,
            &["config", "branch.issue/7-x.wtnIssue", "other/thing#9"],
        );
        let close = |repo: &Repo, n: u64, _: &str, _: &str| {
            assert_eq!((repo.to_string(), n), ("other/thing".to_string(), 9));
            Err("GitHub is off in Settings".to_string())
        };
        let given = Given {
            close_issue: true,
            ..given_for(&wt)
        };
        let out = run(&given, &notes, &close).unwrap();
        assert!(
            out.contains(&"left open: other/thing#9: GitHub is off in Settings".to_string()),
            "{out:?}"
        );
        assert!(!wt.exists(), "removed all the same");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_branch_named_for_an_issue_closes_nothing_and_one_not_asked_to_is_left_open() {
        // Named like one, but no issue wtn recorded: another tracker's number.
        let (dir, _main, wt, notes) = issue_setup("named", "issue/42-fix-login");
        let given = Given {
            close_issue: true,
            ..given_for(&wt)
        };
        let out = run(&given, &notes, &never).unwrap();
        assert!(out.iter().all(|l| !l.contains("#42")), "{out:?}");
        let _ = std::fs::remove_dir_all(&dir);

        let (dir, main, wt, notes) = issue_setup("leak", "issue/12-x");
        git(
            &main,
            &["config", "branch.issue/12-x.wtnIssue", "acme/widgets#12"],
        );
        let given = Given {
            close_issue: true,
            // Made here, so the repository's own leak-check finds no path.
            says: format!("It reads /{}/marta/secret.txt now.", "Users"),
            ..given_for(&wt)
        };
        let out = run(&given, &notes, &never).unwrap();
        assert!(
            out.iter()
                .any(|l| l.starts_with("left open: acme/widgets#12: leak-check found")),
            "a home path stays here: {out:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);

        let (dir, main, wt, notes) = issue_setup("unasked", "issue/12-x");
        git(
            &main,
            &["config", "branch.issue/12-x.wtnIssue", "acme/widgets#12"],
        );
        let out = run(&given_for(&wt), &notes, &never).unwrap();
        assert!(
            out.contains(
                &"left open: acme/widgets#12: --close-issue not given, so not asked to close it"
                    .to_string()
            ),
            "{out:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
