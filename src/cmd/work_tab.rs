//! The one module that starts work tabs: a tab running `wt`, `wtb` or
//! `wtpr` in a repository, for a source's item (a Linear ticket, a GitHub
//! issue, a note's checklist item, a branch, a task, a PR), and what those
//! commands share once they run in it. No tmux-version file is behind it:
//! it gathers what new-tab.zsh, wt-open.zsh, wtb-open.zsh, wtpr-open.zsh
//! and wtn-open.zsh each did for themselves.
//!
//! On the server, `Server::open_work_tab` opens every work tab, whoever
//! asks (the workspace's keys, mission control's menus, the Extras view,
//! autopilot), from a `Work`, whose `command` is also what `wt-batch` asks
//! the server to run in each tab. Each tab runs its command as an
//! interactive shell of yours that defines the command's shell function
//! from its `init` (`worktree::tab_running`), so the shell stays in the
//! worktree once Claude exits.
//!
//! In the tab, each command (worktree_from_ticket.rs, worktree_from_issue.rs,
//! worktree_from_note.rs, worktree_from_branch.rs, worktree_from_pr.rs)
//! reads its source and names the branch, makes the worktree through
//! `worktree::add_worktree` (`ensure_worktree` for `wt`'s, `make_worktree`
//! for an item's and an issue's, cut from `start_point`), and hands it to
//! Claude through `hand_over`. What an item's and an issue's starts share
//! besides (the target they land on, what the branch records, the tab
//! autopilot opened them in) is here too.

use std::fs;
use std::path::Path;
use std::process::ExitCode;

use crate::autopilot::mode::{Mode, Sources};
use crate::mux::TabId;
use crate::mux::server::{ClientId, Opened, Server};
use crate::paths;
use crate::worktree::{self, Given, Purpose};

/// What a work tab works on, as its command runs it. Only a ticket's ID,
/// an issue's reference, an item's line, a branch's name, a task's words
/// or a PR's address is ever in one, each a word of its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Work {
    /// A Linear ticket's worktree (`wt <ticket>`).
    Ticket(String),
    /// A GitHub issue's (`wt --issue <owner>/<repo>#<number>`), cut from
    /// and landing on `target`, empty for the default branch.
    Issue { reference: String, target: String },
    /// A note's checklist item's (`wt --item <line>`, the note's path in
    /// WT_NOTE), by its line and first line, with `target` as an issue's.
    Item {
        note: String,
        line: usize,
        text: String,
        target: String,
    },
    /// A branch's, made when it is nowhere (`wtb <branch>`).
    Branch(String),
    /// A task typed in, on a new branch named after it (`wtb --prompt`).
    Task(String),
    /// A PR's (`wtpr <url>`), with a first prompt of its own when there is
    /// one (WTPR_PROMPT).
    Pr { url: String, prompt: Option<String> },
    /// No worktree: Claude shaping a Linear ticket with you (`shape`).
    Shape(String),
}

/// `wt`'s `--target` and its branch, none for the default branch.
fn with_target(mut args: Vec<String>, target: &str) -> Vec<String> {
    if !target.is_empty() {
        args.extend(["--target".to_string(), target.to_string()]);
    }
    args
}

impl Work {
    /// The new tab's command: an interactive shell of yours that runs the
    /// work and stays a shell once it ends.
    pub fn command(&self) -> Vec<String> {
        let wt = |args: Vec<String>| worktree::tab_running("worktree-from-ticket", "wt", &args);
        let wtb = |args: Vec<String>| worktree::tab_running("worktree-from-branch", "wtb", &args);
        match self {
            Work::Ticket(ticket) => wt(vec![ticket.clone()]),
            Work::Issue { reference, target } => wt(with_target(
                vec!["--issue".to_string(), reference.clone()],
                target,
            )),
            Work::Item {
                note, line, target, ..
            } => {
                let mut command = vec!["env".to_string(), format!("WT_NOTE={note}")];
                command.extend(wt(with_target(
                    vec!["--item".to_string(), line.to_string()],
                    target,
                )));
                command
            }
            Work::Branch(branch) => wtb(vec![branch.clone()]),
            Work::Task(task) => wtb(vec!["--prompt".to_string(), task.clone()]),
            Work::Pr { url, prompt } => {
                let run =
                    worktree::tab_running("worktree-from-pr", "wtpr", std::slice::from_ref(url));
                match prompt {
                    Some(prompt) => ["env".to_string(), format!("WTPR_PROMPT={prompt}")]
                        .into_iter()
                        .chain(run)
                        .collect(),
                    None => run,
                }
            }
            Work::Shape(ticket) => {
                let script = format!("{} \"$@\"", worktree::self_command("shape"));
                worktree::tab_command(&script, "shape", std::slice::from_ref(ticket))
            }
        }
    }
}

impl Server {
    /// Opens a tab for `work` at the end of the client's session, in `dir`,
    /// in front or behind as `opened` says; says why when it can't.
    pub fn open_work_tab(
        &mut self,
        client: ClientId,
        dir: &Path,
        work: Work,
        opened: Opened,
    ) -> Option<TabId> {
        let session = self.clients.get(&client)?.session.clone();
        match self.open_work_tab_in(&session, dir, work, opened) {
            Ok(tab) => Some(tab),
            Err(e) => {
                self.say(Some(client), &e);
                None
            }
        }
    }

    /// `open_work_tab` in a session named, for a caller with no client (the
    /// phone app), the error its words. An item's tab is named after the
    /// item, cut short, its whole first line its full title, and linked to
    /// its note (`@tab_name`, `@tab_title`, `@tab_note`); every other tab is
    /// named once its command has made the worktree, from its branch
    /// (src/tabs/names.rs).
    pub fn open_work_tab_in(
        &mut self,
        session: &str,
        dir: &Path,
        work: Work,
        opened: Opened,
    ) -> Result<TabId, String> {
        let (tab, _) = self
            .new_tab(
                session,
                Some(dir.to_path_buf()),
                Some(work.command()),
                None,
                opened,
            )
            .map_err(|e| format!("Couldn't open a tab: {e}"))?;
        self.tab_to_end(tab);
        if let Work::Item { note, text, .. } = work
            && let Some(t) = self.tabs.get_mut(&tab)
        {
            t.opts
                .insert("@tab_name".into(), crate::tabs::title::short_name(&text));
            t.opts.insert("@tab_title".into(), text);
            t.opts.insert("@tab_note".into(), note);
            self.dirty_tab(tab);
        }
        Ok(tab)
    }
}

// ── In the tab: starting the work ───────────────────────────────────────────

/// The commit a new item's or issue's branch starts from: origin's default branch as it
/// is now, fetched first (else the main clone's HEAD, with no remote), never
/// the worktree /wt was run in, whose own commits would then land with this
/// item's. A fetch that fails is tried twice more (`worktree::FETCH_RETRIES_S`), then
/// the branch is cut from origin's default branch as last fetched, said with
/// its age, since the landing rebases onto the newest one anyway; with none
/// fetched before it stops, since the main clone's unpushed commits would
/// otherwise land with the item.
pub(crate) fn start_point(root: &Path, target: Option<&str>) -> Result<String, String> {
    if let Some(target) = target {
        return target_start(root, target);
    }
    let default = crate::repos::default_branch(root);
    let name = default.clone().unwrap_or_else(|| {
        worktree::origin_head(root)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "main".into())
    });
    let fetched = worktree::with_retries(
        &worktree::fetch_waits(),
        || eprintln!("Fetching origin's {name} again"),
        || {
            if let Some(default) = &default {
                point_origin_head_at(root, default)?;
            }
            worktree::default_start(root, "wt: fetch the base")
        },
    );
    fetched.or_else(|e| {
        worktree::last_fetched(root, default.as_deref())
            .map(|(r, _)| r)
            .ok_or(e)
    })
}

/// Points origin's `HEAD` in the clone at `default`, the default branch
/// `switchboard landing --json` names (`repos::default_branch`), when it
/// names another, fetching that branch first: the branch then starts where
/// queue-and-clean lands it, and the push rule, read from the start, allows
/// the push it makes. A fetch or update that fails stops it, since starting
/// from the branch `HEAD` still names would bring the mismatch back.
pub(crate) fn point_origin_head_at(root: &Path, default: &str) -> Result<(), String> {
    let head = worktree::git_at(
        root,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .ok();
    let names_another = head.as_deref() != Some(&format!("origin/{default}"));
    if !names_another || !worktree::remotes(root).iter().any(|r| r == "origin") {
        return Ok(());
    }
    if !worktree::fetch_branch(root, "origin", default, "wt: fetch the base") {
        return Err(format!(
            "couldn't fetch origin's {default}, the default branch the landing names, which the new branch starts from"
        ));
    }
    if !worktree::git_ok(root, &["remote", "set-head", "origin", default]) {
        return Err(format!("couldn't point origin/HEAD at origin's {default}"));
    }
    Ok(())
}

/// A target's branch fetched from `remote`, tried twice more as the base's
/// fetch is (`worktree::FETCH_RETRIES_S`): whether it was.
fn fetch_target(root: &Path, remote: &str, branch: &str) -> bool {
    worktree::with_retries(
        &worktree::fetch_waits(),
        || eprintln!("Fetching {remote}/{branch} again"),
        || {
            worktree::fetch_branch(root, remote, branch, "wt: fetch the target")
                .then_some(())
                .ok_or_else(String::new)
        },
    )
    .is_ok()
}

/// The commit an item's branch starts from when it lands on `target`, a
/// local branch (made from origin's when only origin has it): the remote
/// branch it tracks as it is now, fetched first, since the landing rebases
/// onto that and pushes there, so the work starts where it will land; the
/// local branch itself when it tracks none. A local branch with commits its
/// remote branch lacks is refused, since the landing would never push them
/// and couldn't fast-forward it. A fetch that fails, tried twice more, is said and the local
/// branch used, since the landing fetches again.
fn target_start(root: &Path, target: &str) -> Result<String, String> {
    let local = format!("refs/heads/{target}");
    let exists = |r: &str| worktree::git_ok(root, &["rev-parse", "--verify", "--quiet", r]);
    // One on origin alone, typed for "Another branch…", made a local
    // branch tracking it.
    if !exists(&local)
        && worktree::remotes(root).iter().any(|r| r == "origin")
        && fetch_target(root, "origin", target)
        && exists(&format!("refs/remotes/origin/{target}"))
    {
        worktree::git_ok(
            root,
            &[
                "branch",
                "--quiet",
                "--track",
                target,
                &format!("origin/{target}"),
            ],
        );
    }
    if !exists(&local) {
        return Err(format!(
            "no branch {target} here or on origin, which autopilot lands this item on"
        ));
    }
    let Some((remote, branch)) = tracked(root, target) else {
        return Ok(local);
    };
    if !fetch_target(root, &remote, &branch) {
        eprintln!("Couldn't fetch {remote}/{branch}; starting from {target} as it is here");
        return Ok(local);
    }
    let fetched = format!("refs/remotes/{remote}/{branch}");
    let ahead = worktree::git_at(
        root,
        &["rev-list", "--count", &format!("{fetched}..{local}")],
    )
    .map_err(|e| e.to_string())?;
    if ahead != "0" {
        let commits = if ahead == "1" { "commit" } else { "commits" };
        let refspec = if branch == target {
            target.to_string()
        } else {
            format!("{target}:{branch}")
        };
        return Err(format!(
            "{target} has {ahead} {commits} {remote}/{branch} doesn't, which the landing wouldn't push; \
             push them (`git push {remote} {refspec}`) or move {target} back, then start the item again"
        ));
    }
    Ok(fetched)
}

/// The remote branch a local branch tracks, by its remote and its name
/// there, from git's config; `None` for a branch that tracks none.
pub fn tracked(root: &Path, branch: &str) -> Option<(String, String)> {
    let get = |key: &str| {
        worktree::git_at(
            root,
            &["config", "--get", &format!("branch.{branch}.{key}")],
        )
        .ok()
        .filter(|v| !v.is_empty())
    };
    let remote = get("remote").filter(|r| r != ".")?;
    let merge = get("merge")?;
    Some((
        remote,
        merge
            .strip_prefix("refs/heads/")
            .unwrap_or(&merge)
            .to_string(),
    ))
}

/// Refuses work from `source` in this clone's repository while autopilot is
/// on there and takes its work from the other source, so the two never
/// both feed it.
pub fn refused_by_source(source: Sources) -> Result<(), String> {
    let root = worktree::repo_root().map_err(|e| e.to_string())?;
    let refusal = super::work::main_clone(&root)
        .and_then(|clone| Mode::read(&paths::state_dir()).refusal(&clone, source));
    match refusal {
        Some(why) => Err(why.to_string()),
        None => Ok(()),
    }
}

/// The branch an item's worktree is cut from and lands on, as `--target`
/// names it: none for the default branch, and refused, but for the default
/// branch named, in a repository that lands through PRs, whose PRs go to the
/// default branch.
pub(crate) fn target_of<'a>(given: &'a Given, root: &Path) -> Result<Option<&'a str>, String> {
    Ok(match given.last("--target").filter(|t| !t.is_empty()) {
        Some(target) if crate::repos::landing_for(root)?.lands != crate::repos::Way::Direct => {
            // Where PRs land, the default branch named is the default
            // branch, cut from origin's as it is now with no target
            // recorded, since that is where its PR goes; any other is
            // refused.
            let default = crate::repos::default_branch(root);
            if default.as_deref() != Some(target) {
                return Err(format!(
                    "{} lands through PRs, so an item cut from {target} would land on the default branch through a PR, carrying {target}'s commits. \
                     Start it without --target, or choose Straight to the default branch for the repository in Settings' Repositories",
                    paths::tilde(root)
                ));
            }
            None
        }
        target => target,
    })
}

/// The worktree at `path` on `branch`, cut from `start` (`start_point`),
/// for `wt`'s work, `work` saying what it is (`item`, `issue`) for git's
/// messages.
pub(crate) fn make_worktree(
    root: &Path,
    path: &Path,
    branch: &str,
    work: &'static str,
    start: &str,
) -> Result<(), String> {
    let purpose = Purpose {
        work,
        command: "wt",
        start: worktree::Start {
            at: Some(start),
            tracks: None,
        },
    };
    worktree::ensure_worktree(root, path, branch, &purpose)
}

/// What a branch keeps once its work is claimed: when it started, for the
/// time its Done line says it took (`switchboard landing`'s `took`, kept
/// when a rerun reuses it), and the branch it lands on, read by
/// `switchboard landing` for queue-and-clean (none, the default branch).
pub(crate) fn after_claim(root: &Path, branch: &str, target: Option<&str>) -> Result<(), String> {
    let started_key = format!("branch.{branch}.wtnStarted");
    if worktree::git_at(root, &["config", "--get", &started_key])
        .unwrap_or_default()
        .is_empty()
    {
        worktree::git_config(root, &[&started_key, &crate::util::now_secs().to_string()]);
    }
    let target_key = format!("branch.{branch}.wtnTarget");
    let recorded_target = match target {
        Some(t) => worktree::git_config(root, &[&target_key, t]),
        None => {
            worktree::git_config(root, &["--unset", &target_key]);
            true
        }
    };
    if recorded_target {
        Ok(())
    } else {
        Err(format!(
            "could not record the target in git config ({target_key})"
        ))
    }
}

/// Where the work lands, in the prompt's words.
pub(crate) fn onto_words(target: Option<&str>) -> String {
    match target {
        Some(t) => format!(
            "{t}, the branch it was cut from (its target, which `switchboard landing` names)"
        ),
        None => "the default branch".to_string(),
    }
}

/// `--prompt`'s words added to an item's or an issue's prompt.
pub(crate) fn also_from_me(given: &Given, prompt: &mut String) {
    let extra = worktree::one_line(&given.all("--prompt").join(" "));
    if !extra.is_empty() {
        prompt.push_str(&format!(" Also, from me for this item: {extra}"));
    }
}

/// The one way every work command hands its worktree to Claude, once it is
/// made and checked: the main clone's local settings linked in, its build
/// cloned (`clone_build`), then Claude told `prompt` (none when empty),
/// with `settings` for its session (`push_rule`), through the cd-file and
/// settings-file the shell function reads, or by starting Claude here. A
/// step that fails takes back what `making` says the start made, since no
/// Claude starts.
pub(crate) fn hand_over(
    given: &Given,
    root: &Path,
    path: &Path,
    prompt: &str,
    settings: Option<String>,
    making: Option<&worktree::Making>,
) -> Result<ExitCode, String> {
    let undo = |e: String| {
        if let Some(making) = making {
            making.undo();
        }
        e
    };
    worktree::link_local_settings(root, path).map_err(|e| undo(e.to_string()))?;
    crate::cmd::clone_build::start(path);
    // Read by the shell function: line 1 the worktree, the rest the prompt.
    if let Some(cd_file) = &given.cd_file {
        worktree::write_cd_file(cd_file, &format!("{}\n{prompt}\n", path.display()))
            .map_err(|e| undo(e.to_string()))?;
    }
    // Read by `wt`'s too: the settings its Claude starts with, empty for none.
    if let Some(settings_file) = given.last("--settings-file") {
        fs::write(settings_file, settings.as_deref().unwrap_or_default())
            .map_err(|e| undo(format!("couldn't write {settings_file}: {e}")))?;
    }
    if given.no_claude {
        return Ok(ExitCode::SUCCESS);
    }
    let mut args = Vec::new();
    if let Some(settings) = settings {
        args.extend(["--settings".to_string(), settings]);
    }
    // After `--`, so a prompt with a leading dash is never one of Claude's
    // flags.
    if !prompt.is_empty() {
        args.extend(["--".to_string(), prompt.to_string()]);
    }
    Ok(worktree::run_in("claude", &args, path))
}

/// Whether the tab this runs in is autopilot's now: opened by it
/// (`@autopilot`) and not made yours since (`@autopilot_yours`).
pub(crate) fn in_autopilot_tab() -> bool {
    use crate::mux::socket::tab_option;
    tab_option(crate::autopilot::OPENED).is_some()
        && tab_option(crate::autopilot::YOURS).is_none_or(|v| v.is_empty())
}

/// Whether the tab this runs in is one of autopilot's, hands-free
/// (`@autopilot_handsfree`, which autopilot sets as it opens the tab).
pub(crate) fn in_hands_free_tab() -> bool {
    crate::mux::socket::tab_option(crate::autopilot::HANDS_FREE).as_deref() == Some("1")
}

/// In a tab autopilot opened (`@autopilot`), why wt refused its item, set
/// as the tab's `@autopilot_refused`, so autopilot skips the item with that
/// reason and closes the tab rather than leave a slot waiting at a shell.
pub(crate) fn refused_in_autopilot(why: &str) {
    if crate::mux::socket::tab_option(crate::autopilot::OPENED).is_some() {
        let _ = crate::mux::socket::ask(&["set", crate::autopilot::REFUSED, why]);
    }
}

/// In a tab autopilot opened (`@autopilot`), the first line of why wt
/// stopped before making the item's worktree, set as the tab's `@autopilot_failed`, so
/// autopilot closes the tab and opens the item again later rather than
/// leave it holding the item at a shell; not over a refusal, which skips
/// the item at once.
pub(crate) fn failed_in_autopilot(why: &str) {
    use crate::mux::socket::{ask, tab_option};
    if tab_option(crate::autopilot::OPENED).is_none()
        || tab_option(crate::autopilot::REFUSED).is_some_and(|r| !r.is_empty())
    {
        return;
    }
    let first = why.lines().next().unwrap_or(why);
    let _ = ask(&["set", crate::autopilot::FAILED, first]);
}

/// The settings a tab of autopilot's starts its Claude with, for that
/// session alone: the command that makes queue-and-clean's landing push,
/// `switchboard land-lock push` (src/cmd/land_push.rs), allowed, so Claude
/// Code's auto mode, which takes an allow rule before its own judgment,
/// doesn't refuse a push the command's own gate decides.
/// Only in a tab autopilot opened (`@autopilot`), in a repository landing
/// straight on a branch that is on a remote, with `autopilot_push_rule` on:
/// every other tab and push is judged as before. Whether the tab lands
/// without asking isn't read here, since that is turned on and off while
/// it works: the command reads it as it pushes, and the server asks the
/// push after your yes (`--asked`) at its PreToolUse hook while it is off
/// (src/agent/hands_free.rs's `ask_push`). It is passed on the command
/// line (`claude --settings`), so it is in no file and no other session
/// has it; the `wt` function, which starts Claude itself, reads it back
/// from `--settings-file`.
pub(crate) fn push_rule(
    path: &Path,
    root: &Path,
    start: &str,
    target: Option<&str>,
) -> Option<String> {
    if !crate::settings::is_on("autopilot_push_rule") || !in_autopilot_tab() {
        return None;
    }
    // The default branch on origin, which the item's branch starts from; a
    // target, the remote branch it tracks. With no remote the landing moves
    // a local branch, which needs no rule.
    match target {
        Some(target) => {
            tracked(root, target)?;
        }
        None => {
            start.strip_prefix("refs/remotes/origin/")?;
        }
    }
    let lands_direct =
        crate::repos::landing_for(path).is_ok_and(|l| l.lands == crate::repos::Way::Direct);
    lands_direct.then(push_settings)
}

/// Claude Code's settings allowing `switchboard land-lock push`, the
/// command queue-and-clean's step 9 runs, exactly so, and nothing else:
/// no `git push`, and not its `--asked` form, which pushes past its gate.
pub(crate) fn push_settings() -> String {
    serde_json::json!({
        "permissions": { "allow": ["Bash(switchboard land-lock push)"] }
    })
    .to_string()
}

/// What every tab autopilot opens is told first, from any source: look
/// for the same work started or done elsewhere, and stop on finding it,
/// marking the tab so autopilot skips the item with that reason
/// (`switchboard autopilot-duplicate`, which sets `@autopilot_refused`)
/// and moves on. Empty in a tab autopilot didn't
/// open, where you decide. `note` names the note the item is in, whose other
/// items are looked at too while Settings' tab_note_context is on; an
/// issue's tab is pointed at no note.
pub(crate) fn duplicates_first(note: Option<&Path>) -> String {
    if !in_autopilot_tab() {
        return String::new();
    }
    // A note's other items only while Settings shares the tab's note with
    // its Claude (`tab_note_context`): nothing in a note goes to Claude
    // unasked.
    let places = match note.filter(|_| crate::settings::is_on("tab_note_context")) {
        Some(file) => format!(
            "the other ticked and open items of my note ({}), ",
            file.display()
        ),
        None => String::new(),
    };
    // GitHub only while Switchboard may reach it.
    let github = if crate::settings::is_on("github") {
        "the repository's open and merged PRs and its GitHub issues, and "
    } else {
        ""
    };
    format!(
        "Before starting, look for the same work started or done elsewhere: {places}{github}{DUPLICATES}"
    )
}

const DUPLICATES: &str = "the open tabs (`switchboard tabs`). \
     When another item, issue, PR or tab already has it, stop without changing anything: run `switchboard autopilot-duplicate '<which>'` and say which, so autopilot skips this one with that reason and moves on. \
     Work already on the branch it lands on is not a duplicate: the check below covers it. ";
