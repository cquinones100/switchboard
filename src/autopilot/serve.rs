//! The mode on the server: what lib/sidebar/daemon.js did for it (M-p's
//! `autopilotStart` and `autopilotSetup`, `pilotMenu`, `pilot.check` on
//! every refresh, its notice) and what tmux/wtn-open.zsh's `next` did, done
//! in the server task.
//!
//! M-p's "Autopilot: work a note" opens mission control on its tabs,
//! then, with the mode off, asks which note (those holding an unticked
//! item, the one last worked first, then the newest, the rest behind "More
//! notes…"), how many of its tabs work at once, the most open and a
//! spend limit, each typed in a box with what it was before, Enter keeping
//! it; the repository is the main clone of the pane it was asked from. With
//! the mode on, it opens mission control on the mode and nothing over it, its
//! menu (`m`) a key away. With UX 2.0 on, setting it up is its settings box
//! instead (src/autopilot/settings_box.rs), which the view's menu opens while
//! it is on, checked and written here (`autopilot_box_write`).
//!
//! Every tick reads `autopilot.json`, gathers the tabs it opened (their
//! directories from what the server holds and git's files, their state from
//! the sidebar's model, or from the pane parked behind M-d's diff), and
//! checks (`Pilot::check`). Reading the queue starts git, so it runs off the
//! task and answers with `Event::Autopilot`; the tab for the item found is
//! opened back on it, in the session used last, as the palette's "Start
//! work on a note's checklist items" opens one (`Server::open_item_tab`).
//!
//! The note is followed by its stable ID (crates/notes/src/id.rs): each tick reads
//! the ID at the note's path (its last bytes, again only once the file
//! changes), and what following it needs, giving it an ID, looking for it
//! by its ID, giving older tabs their note's ID, bringing wtn's claims to
//! IDs, runs off the task too (`Keep`), its answer written back to the
//! mode.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use super::line::{self, Act, Summary};
use super::mode::{self, Mode, note_title};
use super::pilot::{Job, Keep, Kept, Mark};
use super::place::{Mine, in_main_clone, in_worktree_of};
use super::settings_box::{self, Context, Fields, Part, SettingsBox};
use super::{AutopilotEvent, Command};
use crate::agent::State;
use crate::cmd::work::main_clone;
use crate::cmd::work_recent;
use crate::cmd::worktree_from_note::{self, Item};
use crate::diff::in_place::PARKED_PANE;
use crate::mission::Mission;
use crate::mission::ask::Ask;
use crate::mux::server::{ClientId, Event, Server};
use crate::mux::{PaneId, TabId};
use crate::sidebar::model::TabState;
use crate::ui::{Menu, MenuOption, Outcome, PickItem, Picker};
use crate::{paths, settings, util};

/// When it opened one of its tabs, in seconds.
pub const OPENED: &str = "@autopilot";
/// Seen in its worktree.
pub const IN: &str = "@autopilot_in";
/// The first line of the item it was opened for.
pub const ITEM: &str = "@autopilot_item";
/// The ID of that item's note, so the item is held in its own note alone.
pub const NOTE: &str = "@autopilot_note";
/// When it was found back in the main clone once landed, in seconds.
pub const LEFT: &str = "@autopilot_left";
/// Seen since it landed.
pub const SEEN: &str = "@autopilot_seen";
/// To land on the default branch without asking to push, which
/// queue-and-clean reads (`switchboard show @autopilot_land`).
pub const LAND: &str = "@autopilot_land";
/// Landed without asking, the commit it pushed, which queue-and-clean sets
/// (`switchboard land-lock autolanded`).
pub const AUTOLAND: &str = "@autopilot_autoland";
/// To take the recommended option of what it would ask (hands-free), which
/// the server reads as its Claude's hooks ask (src/agent/hands_free.rs).
pub const HANDS_FREE: &str = "@autopilot_handsfree";
/// How many times hands-free has had its Claude carry on past a turn that
/// ended on a question.
pub const NUDGED: &str = "@autopilot_nudged";
/// Pushed, asked about or not, the commit it pushed, which queue-and-clean
/// sets as soon as its push succeeds, so hands-free sends no turn back after.
pub const PUSHED: &str = "@autopilot_pushed";
/// When it landed, in seconds: the server sets it as it first sees
/// `@autopilot_pushed` or `@autopilot_autoland` set on one of its tabs
/// (`landed_at`), and never again, so the board's timer stops there.
pub const LANDED_AT: &str = "@autopilot_landed_at";
/// Landed and made yours by a prompt typed in it since: the mode lets go
/// of it, while its item stays under the view's Landed.
pub const YOURS: &str = "@autopilot_yours";
/// Why wtn refused the tab's item (its claim check), which wtn sets as it
/// exits: the item is skipped and the tab closed, so no slot waits at the
/// shell wtn leaves.
pub const REFUSED: &str = "@autopilot_refused";
/// The GitHub issue the tab works, `<owner>/<repo>#<number>`, which wtn
/// sets as it starts one (src/cmd/worktree_from_issue.rs): commented on and
/// closed once the tab lands, and commented on when it is a duplicate; its
/// claim on GitHub released once it lands or closes before landing.
pub const ISSUE: &str = "@autopilot_issue";
/// Set by wtn once it has claimed its issue on GitHub: a start that fails
/// after it failed here, not on GitHub's say, so it counts toward
/// `pilot::FAILS_SKIP` as a note item's does rather than claiming and
/// releasing again on every retry.
pub const CLAIMED: &str = "@autopilot_claimed";
/// How what it says while GitHub issues can't be read starts
/// (`pilot::unread_said`), a good read taking it back.
const ISSUES_UNREAD: &str = "Can't read GitHub issues";
/// What a failed write on an issue starts with, kept until dismissed.
const WRITE_FAILED: &str = "Couldn't write on ";
/// What a duplicate's reason starts with, kept in what it says until
/// dismissed as a refusal's is.
const DUPLICATE_SAID: &str = ": Duplicate of ";
/// What its notification says of a refused item, after the item.
const REFUSED_SAID: &str = "wtn refused it";
/// Why wtn stopped before making the item's worktree in the tab (a fetch of origin
/// that failed, a worktree it couldn't make), the error's first line, which
/// wtn sets as it exits: the tab is closed and the item opened again on a
/// later check, until it has failed `pilot::FAILS_SKIP` times in an hour.
pub const FAILED: &str = "@autopilot_failed";
/// What its notification says of an item skipped for failing to start.
const FAILED_SAID: &str = "its start failed";
/// A tab of the mode marked with when it landed as `name`, just set to
/// `value`, is its push (`PUSHED` or `AUTOLAND`), the first time only.
pub fn landed_at(
    opts: &mut std::collections::BTreeMap<String, String>,
    name: &str,
    value: &str,
    now_secs: i64,
) {
    let a_push = (name == PUSHED || name == AUTOLAND) && !value.trim().is_empty();
    let of_the_mode = opts.get(OPENED).is_some_and(|v| !v.is_empty() && v != "0");
    if a_push && of_the_mode && !opts.contains_key(LANDED_AT) {
        opts.insert(LANDED_AT.into(), now_secs.to_string());
    }
}

/// The marks of what the mode has a tab do, taken off once it is yours.
const WORKING: [&str; 3] = [LAND, HANDS_FREE, NUDGED];

/// The commit `HEAD` is at in the checkout `dir` is in, read from git's
/// files: a loose ref, `packed-refs`, or a detached `HEAD`'s own.
fn head_of(dir: &Path) -> Option<String> {
    let place = crate::tabs::git::locate(dir)?;
    let head = std::fs::read_to_string(place.git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    let Some(name) = head.strip_prefix("ref:").map(str::trim) else {
        return Some(head.to_string());
    };
    for at in [&place.git_dir, &place.common_dir] {
        if let Ok(sha) = std::fs::read_to_string(at.join(name)) {
            return Some(sha.trim().to_string());
        }
    }
    let packed = std::fs::read_to_string(place.common_dir.join("packed-refs")).ok()?;
    packed
        .lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with('^'))
        .find_map(|l| {
            let (sha, r) = l.split_once(' ')?;
            (r.trim() == name).then(|| sha.to_string())
        })
}

/// What is left of the note for the view's estimate, off the server's
/// task, from the queue `job` gave and those it holds: the whole queue;
/// each item held by open items it builds on, with all of them; and how many wait on you,
/// those held on a design board's review and, unless hands-free takes
/// them, those with an open choice, which the queue leaves out. Nothing
/// with the autopilot_eta setting off.
fn autopilot_left(
    job: &Job,
    notes: &Path,
    state: &Path,
    queue: &[Item],
    held: &[Item],
) -> super::eta::Left {
    if !settings::is_on("autopilot_eta") {
        return super::eta::Left::default();
    }
    let text = std::fs::read_to_string(notes.join(&job.note)).unwrap_or_default();
    let on_review = |i: &Item| {
        let item = worktree_from_note::item_text(&text, i.line);
        matches!(
            worktree_from_note::plan_of(notes, &item),
            worktree_from_note::Plan::Review(_)
        )
    };
    // An issue is held only by a design review (line 0, no note item).
    let (after, reviews): (Vec<&Item>, Vec<&Item>) =
        held.iter().partition(|i| i.line != 0 && !on_review(i));
    let choices = if job.with_choices {
        0
    } else {
        let taken: std::collections::HashSet<&str> =
            queue.iter().chain(held).map(|i| i.text.as_str()).collect();
        let all = Job {
            with_choices: true,
            ..job.clone()
        };
        let (q, h) = all.items_and_held(notes, state).unwrap_or_default();
        q.iter()
            .chain(&h)
            .filter(|i| !taken.contains(i.text.as_str()))
            .count()
    };
    super::eta::Left {
        ready: queue.iter().map(|i| i.text.clone()).collect(),
        after: after
            .into_iter()
            .map(|i| {
                let on = worktree_from_note::held_by_all(&text, i.line);
                (i.text.clone(), on)
            })
            .filter(|(_, on)| !on.is_empty())
            .collect(),
        on_you: reviews.len() + choices,
    }
}

/// When a file last changed, in ms; `None` when it isn't there.
fn modified_ms(path: &Path) -> Option<i64> {
    let at = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(at.duration_since(UNIX_EPOCH).ok()?.as_millis() as i64)
}

/// What says a file changed: its modified time, size and change time
/// (giving a note its ID keeps its modified time, but not its change time).
fn stamp_of(path: &Path) -> Option<super::Stamp> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(path).ok()?;
    Some((
        m.mtime(),
        m.mtime_nsec(),
        m.size(),
        m.ctime(),
        m.ctime_nsec(),
    ))
}

/// What it says of a repository that lands through a PR.
const PR_REFUSED: &str = "Autopilot lands only straight onto a branch for now, and this repository lands through a PR (repos.json)";

/// What the settings box shows besides its fields, read off the task: how
/// the repository typed lands, the repositories and branches it suggests,
/// today's spend, and the note's items left out for needing a choice.
fn box_context(mode: &Mode, fields: &Fields, notes: &Path) -> Context {
    let repo = fields.repo_path();
    let landing = if fields.repo.trim().is_empty() {
        String::new()
    } else {
        crate::repos::landing_for(&repo, false)
            .map(|l| landing_words(&l.landing, &l.source))
            .unwrap_or_else(|e| format!("couldn't tell how it lands: {e}"))
    };
    let mut repos: Vec<String> = Vec::new();
    let mut add = |r: String| {
        if !r.is_empty() && !repos.contains(&r) {
            repos.push(r);
        }
    };
    add(fields.repo.trim().to_string());
    if mode.set_up() {
        add(paths::tilde(&mode.repo));
    }
    for key in util::read_json_object(crate::repos::file()).keys() {
        if let Some(top) = key.strip_suffix("/.git")
            && Path::new(top).is_dir()
        {
            add(paths::tilde(top));
        }
    }
    let mut branches: Vec<String> = Vec::new();
    if let Some(place) = crate::tabs::git::locate(&repo) {
        let c = &place.common_dir;
        let recent = work_recent::branches(c);
        let default = crate::tabs::git::default_branch(c);
        for b in recent
            .into_iter()
            .chain(default)
            .chain(crate::tabs::git::local_branches(c))
        {
            if !b.is_empty() && !branches.contains(&b) {
                branches.push(b);
            }
        }
    }
    let state = paths::state_dir();
    let needs_choice = if fields.hands_free || fields.note.is_empty() || !repo.is_dir() {
        Vec::new()
    } else {
        let skip = fields.skipped.iter().cloned().collect();
        let claims = worktree_from_note::claims_file();
        let take = |with: bool| {
            worktree_from_note::queue(notes, &repo, &fields.note, &skip, false, with, &claims)
                .unwrap_or_default()
                .into_iter()
                .map(|i| i.text)
                .collect::<Vec<_>>()
        };
        let without = take(false);
        take(true)
            .into_iter()
            .filter(|t| !without.contains(t))
            .collect()
    };
    Context {
        on: mode.on,
        paused: mode.paused,
        busy: 0,
        open: 0,
        spent: crate::usage::sessions::spent_today(&state, util::now_ms()),
        weekly: crate::usage::sessions::weekly_used(&state, util::now_ms()).map(|(used, _)| used),
        landing_repo: fields.repo.clone(),
        landing,
        repos,
        branches,
        needs_choice,
        offer_hands_free: settings::is_on("autopilot_handsfree"),
        offer_issues: settings::is_on("autopilot_github_issues") && settings::is_on("github"),
        problem: String::new(),
    }
}

/// The skipped items of the note whose ID is `id`; none for a note with
/// no ID yet.
fn skipped_of(mode: &Mode, id: &str) -> Vec<String> {
    mode.skipped
        .iter()
        .filter(|x| !id.is_empty() && x.note == id)
        .map(|x| x.text.clone())
        .collect()
}

/// How a repository lands, in words, and what says so.
fn landing_words(landing: &str, source: &str) -> String {
    let how = if landing == "direct" {
        "straight on it through queue-and-clean"
    } else {
        "through a PR, which autopilot doesn't take yet"
    };
    format!("{how}{}", said_by(source))
}

/// What said how a repository lands, as words to follow it.
pub fn said_by(source: &str) -> &'static str {
    match source {
        "repos.json" => ", as repos.json says",
        "github" => ", as GitHub's rules say",
        "claude-md" => ", as its CLAUDE.md says",
        _ => "",
    }
}

/// Why `branch` can't be landed on in the repository at `repo`, which
/// lands through PRs, as `source` says.
pub fn target_refused(branch: &str, repo: &Path, source: &str) -> String {
    let name = repo
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| paths::tilde(repo));
    format!(
        "{branch} can't be landed on: {name} lands through PRs{}, and they go to the default branch. Land on the default branch, or set \"landing\": \"direct\" for it in ~/.config/switchboard/repos.json",
        said_by(source)
    )
}

/// A whole number, 1 or more, as typed; empty keeps `was`.
fn whole(text: &str, was: u32) -> Option<u32> {
    if text.is_empty() {
        return Some(was);
    }
    let n: f64 = text.parse().ok()?;
    (n.fract() == 0.0 && (1.0..=f64::from(u32::MAX)).contains(&n)).then_some(n as u32)
}

/// Dollars, as typed, a `$` in front or not; empty for no limit.
fn dollars(text: &str) -> Option<f64> {
    if text.is_empty() {
        return Some(0.0);
    }
    let n: f64 = text.strip_prefix('$').unwrap_or(text).trim().parse().ok()?;
    (n.is_finite() && n >= 0.0).then_some(n)
}

/// The longest `autopilot-start`'s check may take and still start it:
/// shorter than the Slack relay waits for its answer
/// (`relay::mux::START_WAIT`), so the relay never says it didn't start
/// while it still may.
pub const START_LIMIT: std::time::Duration = std::time::Duration::from_secs(240);

/// What is wrong with the fields that needs no look at the files, and the
/// part of the settings box it is about.
fn fields_problem(fields: &Fields) -> Option<(String, Part)> {
    if fields.note.is_empty() {
        return Some(("Pick a note to work".into(), Part::Note));
    }
    if fields.limit().is_none() {
        return Some((
            "Spend limit: dollars, or empty for none".into(),
            Part::Limit,
        ));
    }
    let target = fields.target.trim();
    if !target.is_empty() && mode::typed_branch(target).is_none() {
        return Some((format!("Not a branch name: {target}"), Part::Target));
    }
    if fields.sources().issues() && !super::issues::label_ok(&fields.issue_label) {
        return Some(("Issue label: one label, with no quotes".into(), Part::Label));
    }
    None
}

/// The fields as checked against the files: the repository's main clone,
/// the note to work and its ID (when it is new or the mode starts), and
/// whether the mode starts.
#[derive(Debug)]
pub struct Checked {
    repo: PathBuf,
    note: String,
    note_id: Option<String>,
    starting: bool,
}

/// What the settings box (or the Slack relay's form) saving looks at off
/// the server's task, taken from the fields and the mode as they are.
struct FieldsCheck {
    typed: PathBuf,
    target: String,
    note: String,
    picked: bool,
    starting: bool,
    mode_repo: PathBuf,
    mode_target: String,
    notes: PathBuf,
    github: bool,
}

impl FieldsCheck {
    fn new(fields: &Fields, was: &Fields) -> FieldsCheck {
        let mode = Mode::read(&paths::state_dir());
        // The note picked, else the mode's own as it is now, which Notes
        // may have renamed since the box opened.
        let picked = fields.note != was.note || mode.note.is_empty();
        FieldsCheck {
            typed: fields.repo_path(),
            target: fields.target.trim().to_string(),
            note: if picked {
                fields.note.clone()
            } else {
                mode.note.clone()
            },
            picked,
            starting: !mode.on,
            mode_repo: mode.repo,
            mode_target: mode.target,
            notes: worktree_from_note::notes_dir(),
            github: settings::is_on("github"),
        }
    }

    /// The repository's main clone and how it lands looked at when it is
    /// new or the mode starts, the note given its ID when it is new, and a
    /// branch named checked to be there: what was found, or the problem.
    fn run(self) -> Result<Checked, String> {
        let FieldsCheck {
            typed,
            target,
            note,
            picked,
            starting,
            mode_repo,
            mode_target,
            notes,
            github,
        } = self;
        let repo = main_clone(&typed)
            .ok_or_else(|| format!("Not a repository: {}", paths::tilde(&typed)))?;
        // Its tabs are counted by the repository they work in, so it moves
        // to another only once stopped, as before the box.
        if !starting && repo != mode_repo {
            return Err(format!(
                "Its tabs work in {}: stop it to work another repository",
                paths::tilde(&mode_repo)
            ));
        }
        // How it lands, checked as starting or picking a note did before
        // the box.
        if starting || picked || repo != mode_repo {
            match crate::repos::landing_for(&repo, false).map(|l| l.landing) {
                Ok(l) if l == "direct" => {}
                Ok(_) => return Err(PR_REFUSED.to_string()),
                Err(e) => return Err(format!("Couldn't tell how it lands: {e}")),
            }
        }
        // A note to start on, or picked, must be there to read.
        let note_id = if picked || starting {
            Some(
                crate::notes::id::ensure_id(&notes.join(&note), &notes).map_err(|e| {
                    format!(
                        "Couldn't read the note {}, pick another: {e}",
                        note_title(&note)
                    )
                })?,
            )
        } else {
            None
        };
        let common = crate::tabs::git::locate(&repo).map(|p| p.common_dir);
        // A branch named must be the repository's own, or one origin has,
        // which an item starting makes a local one; one not fetched yet is
        // fetched to tell, as an item starting would fetch it, while
        // Settings' GitHub allows reaching it.
        let known = |c: &Path| {
            crate::tabs::git::local_branches(c).contains(&target)
                || crate::tabs::git::on_origin(c, &target)
        };
        if !target.is_empty()
            && (starting || target != mode_target || repo != mode_repo)
            && !common.as_deref().is_some_and(known)
        {
            if !github {
                return Err(format!(
                    "{} has no branch {target} fetched, and GitHub is off in Settings, so it isn't fetched",
                    paths::tilde(&repo)
                ));
            }
            if !crate::worktree::fetch_branch(&repo, "origin", &target, "autopilot") {
                return Err(format!("{} has no branch {target}", paths::tilde(&repo)));
            }
        }
        if target != mode_target
            && let Some(common) = &common
        {
            work_recent::pick_branch(common, &target);
        }
        Ok(Checked {
            repo,
            note,
            note_id,
            starting,
        })
    }
}

/// A percent of the week, as typed, a `%` after it or not: a whole number
/// from 1 to 100; empty, or 0, for no limit.
fn percent(text: &str) -> Option<u32> {
    let text = text.trim();
    let n: f64 = match text.strip_suffix('%').unwrap_or(text).trim() {
        "" => return Some(0),
        n => n.parse().ok()?,
    };
    (n.fract() == 0.0 && (0.0..=100.0).contains(&n)).then_some(n as u32)
}

impl Server {
    // ── M-p ─────────────────────────────────────────────────────────────────

    /// M-p's command, pressed with `pane` current: mission control on the
    /// Autopilot view, which shows the mode while it is on (its menu and
    /// settings box a key or a click away there, nothing opened over it),
    /// else the mode set up: its settings box with UX 2.0, its questions
    /// without.
    pub fn autopilot_command(&mut self, client: ClientId, command: Command, pane: Option<PaneId>) {
        match command {
            Command::WorkNote => {
                let dir = pane
                    .and_then(|p| self.panes.get(&p))
                    .and_then(|s| s.pane.cwd())
                    .unwrap_or_else(paths::home);
                self.open_mission(client, super::opening());
                if !settings::is_on("autopilot") {
                    self.mission_note(client, "Autopilot is off in Settings (Autopilot)");
                    return;
                }
                if Mode::read(&paths::state_dir()).on {
                    return;
                }
                if crate::sidebar::text::ux2() {
                    self.autopilot_box(client, Some(dir), None, Part::Note);
                } else {
                    self.autopilot_setup(client, &dir);
                }
            }
        }
    }

    /// Which note, which branch to land on, how many tabs at once, the most
    /// open and a spend limit, then the mode on, working the note in the
    /// main clone of `dir`'s repository. A repository that lands through a
    /// PR is refused: autopilot lands only straight onto a branch for now.
    fn autopilot_setup(&mut self, client: ClientId, dir: &Path) {
        let Some(repo) = main_clone(dir) else {
            self.mission_note(
                client,
                "Ask from a pane in the note's repository: its items are worked there",
            );
            return;
        };
        let notes = worktree_from_note::notes_dir();
        let was = Mode::read(&paths::state_dir());
        let last = was.note.clone();
        let at = repo.clone();
        self.off_task(
            client,
            move || {
                let landing = crate::repos::landing_for(&at, false).map(|l| l.landing);
                (mode::note_choices(&notes, &last), landing)
            },
            move |server, client, (notes, landing)| {
                match landing.as_deref() {
                    Ok("direct") => {}
                    Ok(_) => {
                        server.mission_note(client, PR_REFUSED);
                        return;
                    }
                    Err(e) => {
                        server.mission_note(client, format!("Couldn't tell how it lands: {e}"));
                        return;
                    }
                }
                if notes.open.is_empty() && notes.rest.is_empty() {
                    server.mission_note(client, "No notes to work");
                    return;
                }
                server.autopilot_pick_note(client, repo, was, notes);
            },
        );
    }

    /// Notes' M-p, the repository picked (`Server::work_note`): mission
    /// control's tabs, then the mode set up for `note` as M-p's command
    /// sets it up once a note is picked: "Land on:", then the counts.
    pub fn autopilot_work_note(&mut self, client: ClientId, repo: &Path, note: String) {
        self.open_mission(client, super::opening());
        if crate::sidebar::text::ux2() {
            self.autopilot_box(client, Some(repo.to_path_buf()), Some(note), Part::Note);
            return;
        }
        let Some(repo) = main_clone(repo) else {
            self.mission_note(client, "That isn't a repository");
            return;
        };
        let was = Mode::read(&paths::state_dir());
        let at = repo.clone();
        self.off_task(
            client,
            move || crate::repos::landing_for(&at, false).map(|l| l.landing),
            move |server, client, landing| {
                match landing.as_deref() {
                    Ok("direct") => {}
                    Ok(_) => {
                        server.mission_note(client, PR_REFUSED);
                        return;
                    }
                    Err(e) => {
                        server.mission_note(client, format!("Couldn't tell how it lands: {e}"));
                        return;
                    }
                }
                let at = repo.clone();
                server.autopilot_pick_target(client, at, move |server, client, target| {
                    server.autopilot_ask_counts(client, repo, note, target, was)
                });
            },
        );
    }

    /// The mode, on and working another note, switched to `note` (M-p's
    /// "Autopilot: work “<note>”" in Notes, said yes to): saved as its
    /// settings box saves another note picked there, everything else kept.
    pub fn autopilot_switch_note(&mut self, client: ClientId, note: String) {
        let mode = Mode::read(&paths::state_dir());
        let notes = worktree_from_note::notes_dir();
        self.open_mission(client, super::opening());
        self.off_task(
            client,
            {
                let path = notes.join(&note);
                move || crate::notes::id::of_file(&path).unwrap_or_default()
            },
            move |server, client, id| {
                let was = Fields::from_mode(&mode);
                let mut fields = was.clone();
                fields.skipped = skipped_of(&mode, &id);
                fields.skipped_of = id;
                fields.note = note;
                server.autopilot_box_write(client, fields, was);
            },
        );
    }

    /// Which note: those holding an unticked item, the rest behind "More
    /// notes…", so the list stays short; with none of them, the rest at
    /// once.
    fn autopilot_pick_note(
        &mut self,
        client: ClientId,
        repo: PathBuf,
        was: Mode,
        notes: mode::NoteChoices,
    ) {
        let (shown, rest) = if notes.open.is_empty() {
            (notes.rest, Vec::new())
        } else {
            (notes.open, notes.rest)
        };
        // A note's value is its path, never empty.
        const MORE: &str = "";
        let mut items: Vec<PickItem> = shown
            .iter()
            .map(|n| PickItem::new(n.strip_suffix(".md").unwrap_or(n), n.clone()))
            .collect();
        if !rest.is_empty() {
            items.push(PickItem::new("More notes…", MORE));
        }
        let picker = Picker::new("Work which note continuously?", items)
            .named("notes")
            .prompt("note> ")
            .keep_order();
        self.pick(client, picker, move |server, client, picked| match picked {
            Some(p) if p.value == MORE => {
                let rest = mode::NoteChoices {
                    open: Vec::new(),
                    rest,
                };
                server.autopilot_pick_note(client, repo, was, rest);
            }
            Some(p) => {
                let note = p.value;
                let at = repo.clone();
                server.autopilot_pick_target(client, at, move |server, client, target| {
                    server.autopilot_ask_counts(client, repo, note, target, was)
                });
            }
            None => {}
        });
    }

    /// "Land on:": the branches picked here before, the most recent first
    /// (src/cmd/work_recent.rs), then the repository's default branch, so
    /// with nothing picked yet Enter lands there, then its other local
    /// branches by name, read from git's files off the task, then "Another
    /// branch…" to type one; `then` given the branch, empty for the default
    /// one, and the pick kept for next time. The palette's "Start work on a
    /// note's checklist items" asks it too, after the items.
    pub(crate) fn autopilot_pick_target(
        &mut self,
        client: ClientId,
        repo: PathBuf,
        then: impl FnOnce(&mut Server, ClientId, String) + Send + 'static,
    ) {
        let repo_for_check = repo.clone();
        self.off_task(
            client,
            move || {
                let common = crate::tabs::git::locate(&repo).map(|p| p.common_dir);
                let (default, branches, recent) = common
                    .as_deref()
                    .map(|c| {
                        let mut branches = crate::tabs::git::local_branches(c);
                        let recent = work_recent::branches(c);
                        // A branch only origin has, picked through "Another
                        // branch…", is made a local one only as an item
                        // starts, so it stays listed while origin has it.
                        let remote_only: Vec<String> = recent
                            .iter()
                            .filter(|b| !b.is_empty() && !branches.contains(b))
                            .filter(|b| crate::tabs::git::on_origin(c, b))
                            .cloned()
                            .collect();
                        branches.extend(remote_only);
                        (crate::tabs::git::default_branch(c), branches, recent)
                    })
                    .unwrap_or_default();
                (common, default, branches, recent)
            },
            move |server, client, (common, default, branches, recent)| {
                let keep = move |branch: &str| {
                    if let Some(common) = common {
                        let branch = branch.to_string();
                        work_recent::in_background(move || {
                            work_recent::pick_branch(&common, &branch)
                        });
                    }
                };
                // A branch other than the default is checked first: a
                // repository landing through PRs sends them to the default
                // branch, so an item cut from another would land there with
                // that branch's commits.
                let at = repo_for_check;
                let default_name = default.clone();
                let allow = move |server: &mut Server, client: ClientId, branch: String| {
                    if branch.is_empty() {
                        keep(&branch);
                        then(server, client, branch);
                        return;
                    }
                    server.off_task(
                        client,
                        move || crate::repos::landing_for(&at, false).map(|l| (l, at)),
                        move |server, client, landing| match landing {
                            Ok((l, _)) if l.landing == "direct" => {
                                keep(&branch);
                                then(server, client, branch)
                            }
                            // The default branch typed by name is where a PR
                            // goes, so it is taken as the default branch.
                            Ok(_) if default_name.as_deref() == Some(branch.as_str()) => {
                                keep("");
                                then(server, client, String::new())
                            }
                            Ok((l, at)) => {
                                server.say(Some(client), &target_refused(&branch, &at, &l.source))
                            }
                            Err(e) => server.say(
                                Some(client),
                                &format!("Couldn't tell how it lands: {e}"),
                            ),
                        },
                    );
                };
                let items = mode::target_choices(default.as_deref(), &branches, &recent)
                    .into_iter()
                    .map(|(label, value)| PickItem::new(label, value))
                    .collect();
                let picker = Picker::new("Land on:", items)
                    .named("branches")
                    .prompt("branch> ");
                server.pick(client, picker, move |server, client, picked| match picked {
                    Some(p) if p.value == mode::ANOTHER_BRANCH => {
                        let ask = Ask::new(
                            "Land on which branch?",
                            "A local branch, or one on origin, which is made a local branch tracking it.",
                        );
                        server.open(
                            client,
                            Box::new(ask),
                            Some(Box::new(move |server: &mut Server, client, outcome| {
                                let Outcome::Picked(p) = outcome else {
                                    return;
                                };
                                match mode::typed_branch(&p.value) {
                                    Some(b) => allow(server, client, b),
                                    None => server.mission_note(
                                        client,
                                        format!("Not a branch name: {}", p.value),
                                    ),
                                }
                            })),
                        );
                    }
                    Some(p) => allow(server, client, p.value),
                    None => {}
                });
            },
        );
    }

    /// The setup's slots and cap, each typed in starting from what it was,
    /// then its spend limit; while the mode is on (its menu's "Change its
    /// note…"), the three kept as they are, since the menu sets each alone.
    fn autopilot_ask_counts(
        &mut self,
        client: ClientId,
        repo: PathBuf,
        note: String,
        target: String,
        was: Mode,
    ) {
        let (slots_were, cap_was, limit_was) = (was.slots, was.cap, was.spend_limit);
        if was.on {
            self.autopilot_begin(
                client,
                repo,
                (note, target),
                (slots_were, cap_was, limit_was),
            );
            return;
        }
        self.autopilot_number(
            client,
            "Tabs working at once",
            "How many of its tabs work at once; one waiting on you doesn't count.",
            slots_were,
            move |server, client, slots| {
                server.autopilot_number(
                    client,
                    "Tabs open at most",
                    "The most of its tabs open at once, those waiting on you included.",
                    cap_was.max(slots),
                    move |server, client, cap| {
                        if cap < slots {
                            server.mission_note(
                                client,
                                format!("Tabs open at most: at least {slots}"),
                            );
                            return;
                        }
                        server.autopilot_ask_limit(
                            client,
                            limit_was,
                            move |server, client, limit| {
                                server.autopilot_begin(
                                    client,
                                    repo,
                                    (note, target),
                                    (slots, cap, limit),
                                )
                            },
                        );
                    },
                );
            },
        );
    }

    /// A whole number asked for in a box, `was` typed in already.
    fn autopilot_number(
        &mut self,
        client: ClientId,
        title: &'static str,
        hint: &'static str,
        was: u32,
        then: impl FnOnce(&mut Server, ClientId, u32) + Send + 'static,
    ) {
        let ask = Ask::new(title, hint).text(was.to_string()).empty_ok();
        self.open(
            client,
            Box::new(ask),
            Some(Box::new(move |server: &mut Server, client, outcome| {
                let Outcome::Picked(p) = outcome else {
                    return;
                };
                match whole(&p.value, was) {
                    Some(n) => then(server, client, n),
                    None => {
                        server.mission_note(client, format!("{title}: a whole number, 1 or more"))
                    }
                }
            })),
        );
    }

    /// The spend limit asked for in a box, `was` typed in already (empty
    /// for none); `then` given it, 0 for none.
    fn autopilot_ask_limit(
        &mut self,
        client: ClientId,
        was: f64,
        then: impl FnOnce(&mut Server, ClientId, f64) + Send + 'static,
    ) {
        let ask = Ask::new(
            "Spend limit",
            "Open no more tabs once today's Claude spend passes this many dollars, as the Usage view counts it; empty for none.",
        )
        .text(if was > 0.0 { was.to_string() } else { String::new() })
        .empty_ok();
        self.open(
            client,
            Box::new(ask),
            Some(Box::new(move |server: &mut Server, client, outcome| {
                let Outcome::Picked(p) = outcome else {
                    return;
                };
                match dollars(&p.value) {
                    Some(limit) => then(server, client, limit),
                    None => server.mission_note(client, "Spend limit: dollars, or empty for none"),
                }
            })),
        );
    }

    /// The weekly limit asked for in a box, `was` typed in already (empty
    /// for none); `then` given it, 0 for none.
    fn autopilot_ask_weekly(
        &mut self,
        client: ClientId,
        was: u32,
        then: impl FnOnce(&mut Server, ClientId, u32) + Send + 'static,
    ) {
        let ask = Ask::new(
            "Weekly limit",
            "Open no more tabs once this percent of Claude Code's weekly rate limit is used, as the statusline last read it; empty for none.",
        )
        .text(if was > 0 { was.to_string() } else { String::new() })
        .empty_ok();
        self.open(
            client,
            Box::new(ask),
            Some(Box::new(move |server: &mut Server, client, outcome| {
                let Outcome::Picked(p) = outcome else {
                    return;
                };
                match percent(&p.value) {
                    Some(limit) => then(server, client, limit),
                    None => server.mission_note(
                        client,
                        "Weekly limit: a percent from 1 to 100, or empty for none",
                    ),
                }
            })),
        );
    }

    /// The note by its ID too, which the mode follows it by, given one when
    /// it has none, then the mode on; off the task, since that writes the
    /// note.
    fn autopilot_begin(
        &mut self,
        client: ClientId,
        repo: PathBuf,
        (note, target): (String, String),
        counts: (u32, u32, f64),
    ) {
        let notes = worktree_from_note::notes_dir();
        let file = notes.join(&note);
        self.off_task(
            client,
            move || crate::notes::id::ensure_id(&file, &notes).ok(),
            move |server, client, note_id| {
                server.autopilot_start(client, repo, (note, target), counts, note_id)
            },
        );
    }

    /// Its menu's "Tabs working at once", "Most tabs open", "Spend limit"
    /// and "Weekly limit": that one box, what it was typed in, and only that count
    /// written. The cap is never below the slots: slots past it raise it,
    /// and a cap typed below them is taken as the slots, each said.
    fn autopilot_set_count(&mut self, client: ClientId, act: Act) {
        let was = Mode::read(&paths::state_dir());
        let set = |server: &mut Server, client, change: Box<dyn FnOnce(&mut Mode) -> String>| {
            let mut said = String::new();
            match Mode::update(&paths::state_dir(), |m| said = change(m)) {
                Ok(_) => {
                    server.autopilot.pilot.reset();
                    server.autopilot_tick();
                    server.mission_note(client, said);
                }
                Err(e) => server.mission_note(client, format!("Couldn't save it: {e}")),
            }
        };
        match act {
            Act::Slots => self.autopilot_number(
                client,
                "Tabs working at once",
                "How many of its tabs work at once; one waiting on you doesn't count.",
                was.slots,
                move |server, client, slots| {
                    set(
                        server,
                        client,
                        Box::new(move |m| {
                            m.slots = slots;
                            if m.cap < slots {
                                m.cap = slots;
                                format!(
                                    "Tabs working at once: {slots}, and most tabs open: {slots}"
                                )
                            } else {
                                format!("Tabs working at once: {slots}")
                            }
                        }),
                    )
                },
            ),
            Act::Cap => self.autopilot_number(
                client,
                "Tabs open at most",
                "The most of its tabs open at once, those waiting on you included.",
                was.cap,
                move |server, client, cap| {
                    set(
                        server,
                        client,
                        Box::new(move |m| {
                            m.cap = cap.max(m.slots);
                            if cap < m.slots {
                                format!("Most tabs open: {}, as many as work at once", m.cap)
                            } else {
                                format!("Most tabs open: {cap}")
                            }
                        }),
                    )
                },
            ),
            Act::WeeklyLimit => {
                self.autopilot_ask_weekly(client, was.weekly_limit, move |server, client, limit| {
                    set(
                        server,
                        client,
                        Box::new(move |m| {
                            // What it said of the limit before no longer
                            // holds once the limit is another.
                            if m.weekly_limit != limit {
                                m.said = super::pilot::unsay(&m.said, super::pilot::WEEKLY);
                            }
                            m.weekly_limit = limit;
                            if limit > 0 {
                                format!("Weekly limit: {limit}% of the week")
                            } else {
                                "Weekly limit: none".to_string()
                            }
                        }),
                    )
                })
            }
            _ => self.autopilot_ask_limit(client, was.spend_limit, move |server, client, limit| {
                set(
                    server,
                    client,
                    Box::new(move |m| {
                        // What it said of the limit before no longer
                        // holds once the limit is another.
                        if m.spend_limit != limit {
                            m.said = super::pilot::unsay(&m.said, super::pilot::SPENT);
                        }
                        m.spend_limit = limit;
                        if limit > 0.0 {
                            format!("Spend limit: ${limit} a day")
                        } else {
                            "Spend limit: none".to_string()
                        }
                    }),
                )
            }),
        }
    }

    /// The mode on, as set up.
    fn autopilot_start(
        &mut self,
        client: ClientId,
        repo: PathBuf,
        (note, target): (String, String),
        (slots, cap, limit): (u32, u32, f64),
        note_id: Option<String>,
    ) {
        let written = Mode::update(&paths::state_dir(), |m| {
            // Skipped items keep their note's ID, so another note's stay
            // skipped in that note alone; one from before notes had IDs,
            // of the note worked before, goes.
            if m.note != note {
                m.skipped.retain(|x| !x.note.is_empty());
            }
            m.on = true;
            m.paused = false;
            m.said.clear();
            m.note = note.clone();
            m.note_id = note_id.unwrap_or_default();
            m.repo = repo.clone();
            m.slots = slots;
            m.cap = cap;
            m.spend_limit = limit;
            m.target = target.clone();
        });
        match written {
            Ok(_) => {
                self.autopilot.pilot.reset();
                self.autopilot_tick();
                self.mission_note(
                    client,
                    format!(
                        "Working {}: {slots} at a time, up to {cap} tabs, in {}, landing on {}",
                        note_title(&note),
                        paths::tilde(&repo),
                        if target.is_empty() {
                            "the default branch"
                        } else {
                            &target
                        }
                    ),
                );
            }
            Err(e) => self.mission_note(client, format!("Couldn't save it: {e}")),
        }
    }

    /// Its menu's "Change the target branch…": "Land on:" asked again, for
    /// the tabs it opens from now; those open land where they were cut from.
    fn autopilot_change_target(&mut self, client: ClientId) {
        let repo = Mode::read(&paths::state_dir()).repo;
        self.autopilot_pick_target(client, repo, |server, client, target| {
            let said = if target.is_empty() {
                "Its new tabs land on the default branch".to_string()
            } else {
                format!("Its new tabs land on {target}")
            };
            match Mode::update(&paths::state_dir(), |m| m.target = target) {
                Ok(_) => {
                    server.autopilot.pilot.reset();
                    server.autopilot_tick();
                    server.mission_note(client, said);
                }
                Err(e) => server.mission_note(client, format!("Couldn't save it: {e}")),
            }
        });
    }

    // ── Its settings in one box (board 35) ──────────────────────────────────

    /// Its settings box, with UX 2.0 on, in place of the chain of boxes and
    /// the menu: the mode's settings, and while it is off the repository of
    /// `dir` and the last note worked (else the first with an unticked
    /// item); `note` in place of the mode's when given (Notes' M-p). The
    /// focus starts on `focus`.
    pub(crate) fn autopilot_box(
        &mut self,
        client: ClientId,
        dir: Option<PathBuf>,
        note: Option<String>,
        focus: Part,
    ) {
        let mode = Mode::read(&paths::state_dir());
        let notes = worktree_from_note::notes_dir();
        self.off_task(
            client,
            move || {
                let mut fields = Fields::from_mode(&mode);
                if !mode.on
                    && let Some(repo) = dir.as_deref().and_then(main_clone)
                {
                    fields.repo = paths::tilde(&repo);
                }
                if fields.note.is_empty() {
                    let choices = mode::note_choices(&notes, &mode.note);
                    fields.note = choices
                        .open
                        .into_iter()
                        .chain(choices.rest)
                        .next()
                        .unwrap_or_default();
                }
                let was = fields.clone();
                if let Some(note) = note.filter(|n| *n != fields.note) {
                    // Its skipped items are the given note's, by its ID.
                    let id = crate::notes::id::of_file(&notes.join(&note)).unwrap_or_default();
                    fields.skipped = skipped_of(&mode, &id);
                    fields.taken_back.clear();
                    fields.skipped_of = id;
                    fields.note = note;
                }
                let ctx = box_context(&mode, &fields, &notes);
                (fields, was, ctx)
            },
            move |server, client, (fields, was, ctx)| {
                server.autopilot_box_open(client, fields, was, ctx, focus)
            },
        );
    }

    /// The box opened again on `fields` as they were typed, after another
    /// question (a note picked, a confirm said no), with `problem` under
    /// the Repository field.
    fn autopilot_box_again(
        &mut self,
        client: ClientId,
        fields: Fields,
        was: Fields,
        problem: String,
        focus: Part,
    ) {
        let mode = Mode::read(&paths::state_dir());
        let notes = worktree_from_note::notes_dir();
        self.off_task(
            client,
            move || {
                let ctx = box_context(&mode, &fields, &notes);
                (fields, ctx)
            },
            move |server, client, (fields, ctx)| {
                let ctx = Context { problem, ..ctx };
                server.autopilot_box_open(client, fields, was, ctx, focus)
            },
        );
    }

    fn autopilot_box_open(
        &mut self,
        client: ClientId,
        fields: Fields,
        was: Fields,
        mut ctx: Context,
        focus: Part,
    ) {
        let summary = self
            .autopilot
            .pilot
            .summary(&self.autopilot.mode, self.autopilot_away());
        ctx.busy = summary.busy;
        ctx.open = summary.open;
        let paused = ctx.paused;
        let boxed = SettingsBox::new(fields, was.clone(), ctx, focus);
        self.open(
            client,
            Box::new(boxed),
            Some(Box::new(move |server: &mut Server, client, outcome| {
                let Outcome::Picked(p) = outcome else {
                    return;
                };
                let Ok(fields) = serde_json::from_str::<Fields>(&p.value) else {
                    return;
                };
                match p.index {
                    settings_box::SAVE => server.autopilot_box_save(client, fields, was),
                    settings_box::PAUSE => server
                        .autopilot_change(client, if paused { Act::Resume } else { Act::Pause }),
                    settings_box::STOP => server.autopilot_change(client, Act::Stop),
                    settings_box::MENU => server.autopilot_menu_list(client),
                    settings_box::NOTE => server.autopilot_box_note(client, fields, was),
                    _ => {}
                }
            })),
        );
    }

    /// The Note field's Enter: every note, those holding an unticked item
    /// first, the box opened again on the one picked (or as it was).
    fn autopilot_box_note(&mut self, client: ClientId, fields: Fields, was: Fields) {
        let notes = worktree_from_note::notes_dir();
        let last = fields.note.clone();
        self.off_task(
            client,
            move || mode::note_choices(&notes, &last),
            move |server, client, choices| {
                let items = choices
                    .open
                    .iter()
                    .chain(&choices.rest)
                    .map(|n| PickItem::new(n.strip_suffix(".md").unwrap_or(n), n.clone()))
                    .collect();
                let picker = Picker::new("Work which note continuously?", items)
                    .named("notes")
                    .prompt("note> ")
                    .keep_order();
                server.pick(client, picker, move |server, client, picked| {
                    let mut fields = fields;
                    let Some(p) = picked.filter(|p| p.value != fields.note) else {
                        server.autopilot_box_again(client, fields, was, String::new(), Part::Note);
                        return;
                    };
                    // The skipped items shown become the picked note's,
                    // found by its ID, read off the task.
                    let file = worktree_from_note::notes_dir().join(&p.value);
                    server.off_task(
                        client,
                        move || crate::notes::id::of_file(&file).unwrap_or_default(),
                        move |server, client, id| {
                            let mode = Mode::read(&paths::state_dir());
                            fields.note = p.value;
                            fields.skipped = skipped_of(&mode, &id);
                            fields.taken_back.clear();
                            fields.skipped_of = id;
                            server.autopilot_box_again(
                                client,
                                fields,
                                was,
                                String::new(),
                                Part::Note,
                            );
                        },
                    );
                });
            },
        );
    }

    /// Its Save (Start while the mode is off): turning Land without asking
    /// or Hands-free on asked first as the menu asks it, a no opening the
    /// box again as it was typed.
    fn autopilot_box_save(&mut self, client: ClientId, fields: Fields, was: Fields) {
        let lands = |f: &Fields| f.land_without_asking || f.hands_free;
        let question = if fields.hands_free && !was.hands_free {
            Some((line::hands_free_question(), Part::HandsFree))
        } else if lands(&fields) && !lands(&was) {
            Some((line::land_question(), Part::Land))
        } else if lands(&fields) && fields.sources().issues() && !was.sources().issues() {
            Some((line::issues_question(), Part::Sources))
        } else {
            None
        };
        let Some(((question, list), part)) = question else {
            self.autopilot_box_write(client, fields, was);
            return;
        };
        self.ask(
            client,
            crate::ui::Confirm::new(question).list(list),
            move |server, client, yes| {
                if yes {
                    server.autopilot_box_write(client, fields, was);
                } else {
                    server.autopilot_box_again(client, fields, was, String::new(), part);
                }
            },
        );
    }

    /// The fields checked and written into the mode at once: the
    /// repository's main clone and how it lands looked at off the task
    /// when it is new or the mode starts, and the note given its ID when
    /// it is new; a problem opens the box again saying it.
    fn autopilot_box_write(&mut self, client: ClientId, fields: Fields, was: Fields) {
        if let Some((problem, part)) = fields_problem(&fields) {
            self.autopilot_box_again(client, fields, was, problem, part);
            return;
        }
        let check = FieldsCheck::new(&fields, &was);
        self.off_task(
            client,
            move || check.run(),
            move |server, client, checked| {
                let checked = match checked {
                    Ok(found) => found,
                    Err(problem) => {
                        server.autopilot_box_again(client, fields, was, problem, Part::Repo);
                        return;
                    }
                };
                match server.autopilot_write_checked(&fields, &was, checked) {
                    Ok(said) => server.mission_note(client, said),
                    Err(e) => server.mission_note(client, e),
                }
            },
        );
    }

    /// The fields, checked by `FieldsCheck::run`, written into the mode,
    /// which starts it while it is off: what to say of it, or why it
    /// couldn't be saved.
    fn autopilot_write_checked(
        &mut self,
        fields: &Fields,
        was: &Fields,
        checked: Checked,
    ) -> Result<String, String> {
        let Checked {
            repo,
            note,
            note_id,
            starting,
        } = checked;
        let m = Mode::update(&paths::state_dir(), |m| {
            fields.apply(m);
            let other_note = m.note != note;
            if other_note || m.repo != repo {
                // What it said of the note or repository before no
                // longer holds, as starting anew clears it.
                m.said.clear();
            }
            if other_note {
                // As starting on another note does.
                m.skipped.retain(|x| !x.note.is_empty());
                m.note = note.clone();
                m.note_id = note_id.clone().unwrap_or_default();
            } else if let Some(id) = &note_id {
                m.note_id = id.clone();
            }
            m.repo = repo.clone();
            if starting {
                m.on = true;
                m.paused = false;
                m.said.clear();
            }
        })
        .map_err(|e| format!("Couldn't save it: {e}"))?;
        self.autopilot.pilot.reset();
        self.autopilot_tick();
        Ok(if starting {
            format!(
                "Working {}: {} at a time, up to {} tabs, in {}, landing on {}",
                note_title(&m.note),
                m.slots,
                m.cap,
                paths::tilde(&m.repo),
                if m.target.is_empty() {
                    "the default branch"
                } else {
                    &m.target
                }
            )
        } else {
            let changes = fields.changes(was);
            if changes.is_empty() {
                "Autopilot: nothing changed".to_string()
            } else {
                format!("Autopilot: {}", changes.join(", "))
            }
        })
    }

    /// `autopilot-start <fields>`, from the Slack relay's Autopilot form
    /// (board 52): the mode, off, started with the fields as its settings
    /// box would start it, answered on `reply` once they are checked off
    /// the task, with what it says of it or why it didn't start; with
    /// `--change`, the mode, on, changed as the box's Save changes it, the
    /// form opened from the Home tab's autopilot section (board 81). Run by
    /// hand it is refused, as autopilot's marks are, since it can turn on
    /// Land without asking and Hands-free, which the box asks a yes for
    /// and the form's Start is.
    pub fn autopilot_start_asked(
        &mut self,
        args: &[String],
        by_hand: bool,
        reply: tokio::sync::oneshot::Sender<crate::mux::server::Answer>,
    ) {
        let answer = |reply: tokio::sync::oneshot::Sender<_>, r| {
            let _ = reply.send(crate::mux::server::Answer::Reply(r));
        };
        let fail = |why: String| crate::mux::server::Reply::fail(why);
        if by_hand {
            answer(
                reply,
                fail("autopilot-start is the Slack relay's, never run by hand".into()),
            );
            return;
        }
        if !settings::is_on("autopilot") {
            answer(
                reply,
                fail("Autopilot is off in Settings (Autopilot)".into()),
            );
            return;
        }
        let change = args.get(1).is_some_and(|a| a == "--change");
        let Some(fields) = args
            .get(if change { 2 } else { 1 })
            .and_then(|a| serde_json::from_str::<Fields>(a).ok())
        else {
            answer(
                reply,
                fail("autopilot-start takes its fields as JSON".into()),
            );
            return;
        };
        let mode = Mode::read(&paths::state_dir());
        if mode.on && !change {
            answer(
                reply,
                // Not naming the note, which the Slack relay may not say.
                fail("Autopilot is already working a note".into()),
            );
            return;
        }
        if !mode.on && change {
            answer(
                reply,
                fail("Autopilot is off, so there is nothing to change".into()),
            );
            return;
        }
        if !fields.note.is_empty() && !crate::notes::serve::in_notes(&fields.note) {
            answer(reply, fail(format!("Not a note: {}", fields.note)));
            return;
        }
        if let Some((problem, _)) = fields_problem(&fields) {
            answer(reply, fail(problem));
            return;
        }
        if fields.hands_free && !mode.hands_free && !settings::is_on("autopilot_handsfree") {
            answer(
                reply,
                fail("Hands-free is off in Settings (Autopilot's hands-free mode)".into()),
            );
            return;
        }
        let was = Fields::from_mode(&mode);
        let check = FieldsCheck::new(&fields, &was);
        let tx = self.tx.clone();
        let asked = std::time::Instant::now();
        tokio::task::spawn_blocking(move || {
            let checked = check.run();
            let _ = tx.send(Event::Autopilot(AutopilotEvent::Started {
                fields: Box::new((fields, was)),
                checked,
                reply,
                asked,
            }));
        });
    }

    /// What `autopilot_start_asked` found, written and answered.
    fn autopilot_started(
        &mut self,
        fields: &Fields,
        was: &Fields,
        checked: Result<Checked, String>,
    ) -> crate::mux::server::Reply {
        use crate::mux::server::Reply;
        // What the check refused, said as it is.
        let checked = match checked {
            Ok(c) => c,
            Err(e) => return Reply::fail(e),
        };
        // Started, or stopped, on the computer meanwhile: left as it is.
        let on = Mode::read(&paths::state_dir()).on;
        if checked.starting && on {
            return Reply::fail("Autopilot was started meanwhile");
        }
        if !checked.starting && !on {
            return Reply::fail("Autopilot was stopped meanwhile");
        }
        match self.autopilot_write_checked(fields, was, checked) {
            Ok(said) => {
                self.say(None, &said);
                Reply::ok(format!("{said}\n"))
            }
            Err(e) => Reply::fail(e),
        }
    }

    // ── Its menu ────────────────────────────────────────────────────────────

    /// The menu `m`, Enter on its line or heading, or its heading's button
    /// in mission control opens, moved through
    /// with j and k as mission control's other menus are.
    pub fn autopilot_menu(&mut self, client: ClientId) {
        if crate::sidebar::text::ux2() {
            self.autopilot_box(client, None, None, Part::Note);
        } else {
            self.autopilot_menu_list(client);
        }
    }

    /// Its menu itself, which the box's `m` opens for the options it
    /// doesn't have (skipping the next item, closing landed tabs).
    fn autopilot_menu_list(&mut self, client: ClientId) {
        let ux2 = crate::sidebar::text::ux2();
        let mode = Mode::read(&paths::state_dir());
        let close = settings::current_settings()
            .into_iter()
            .find(|o| o.key == "autopilot_close")
            .map(|o| o.value_words())
            .unwrap_or_default();
        let list = line::choices(
            &mode,
            &self.autopilot.pilot.next.text,
            &close,
            settings::is_on("autopilot_handsfree"),
        );
        let options = list
            .iter()
            .map(|(_, label, act)| match act {
                Act::Change if ux2 => {
                    MenuOption::new("Change its note, slots, cap or spend limit…")
                }
                _ => MenuOption::new(label.clone()),
            })
            .collect();
        let menu = Menu::new(format!("Working {}", note_title(&mode.note)), options);
        self.choose(client, menu, move |server, client, chose| {
            if let Some((_, _, act)) = chose.and_then(|i| list.into_iter().nth(i)) {
                server.autopilot_act(client, act);
            }
        });
    }

    /// Does what an option of its menu, or of its notification's, says.
    pub fn autopilot_act(&mut self, client: ClientId, act: Act) {
        let state = paths::state_dir();
        // With UX 2.0, its settings box, on the field the option names.
        let field = match act {
            Act::Change => Some(Part::Note),
            Act::ChangeTarget => Some(Part::Target),
            Act::Slots => Some(Part::Slots),
            Act::Cap => Some(Part::Cap),
            Act::SpendLimit => Some(Part::Limit),
            Act::WeeklyLimit => Some(Part::Weekly),
            _ => None,
        };
        if let Some(focus) = field
            && crate::sidebar::text::ux2()
        {
            self.autopilot_box(client, None, None, focus);
            return;
        }
        match act {
            Act::Change => {
                let repo = Mode::read(&state).repo;
                let dir = if repo.as_os_str().is_empty() {
                    paths::home()
                } else {
                    repo
                };
                self.autopilot_setup(client, &dir);
            }
            Act::ChangeTarget => self.autopilot_change_target(client),
            Act::Slots | Act::Cap | Act::SpendLimit | Act::WeeklyLimit => {
                self.autopilot_set_count(client, act)
            }
            Act::ToTabs => self.open_mission(client, super::opening()),
            Act::Dismiss => {
                self.autopilot_dismiss();
                self.mission_note(client, "Dismissed.");
            }
            // The autopilot_close setting to its next choice, as Settings
            // changes it.
            Act::CycleClose => {
                let Some(option) = settings::read_settings()
                    .into_iter()
                    .find(|o| o.key == "autopilot_close")
                else {
                    return;
                };
                if let Err(e) = settings::write_setting("autopilot_close", option.next_value()) {
                    self.mission_note(client, format!("Couldn't save it: {e}"));
                    return;
                }
                let now = settings::reload_settings()
                    .into_iter()
                    .find(|o| o.key == "autopilot_close")
                    .map(|o| o.value_words())
                    .unwrap_or_default();
                self.mission_note(client, format!("Landed tabs close: {now}"));
            }
            // Asked first, naming what it skips and keeps; no is no.
            Act::LandWithoutAsking => {
                let (question, list) = line::land_question();
                self.ask(
                    client,
                    crate::ui::Confirm::new(question).list(list),
                    |server, client, yes| {
                        if yes {
                            server.autopilot_change(client, Act::LandWithoutAsking);
                        }
                    },
                );
            }
            // Asked first too, naming what it decides and what still stops.
            Act::HandsFree => {
                let (question, list) = line::hands_free_question();
                self.ask(
                    client,
                    crate::ui::Confirm::new(question).list(list),
                    |server, client, yes| {
                        if yes {
                            server.autopilot_change(client, Act::HandsFree);
                        }
                    },
                );
            }
            act => self.autopilot_change(client, act),
        }
    }

    /// What an option of its menu changes in the mode, done and said.
    fn autopilot_change(&mut self, client: ClientId, act: Act) {
        let mut said = None;
        match Mode::update(&paths::state_dir(), |m| said = line::apply(&act, m)) {
            Ok(_) => {
                self.autopilot.pilot.reset();
                self.autopilot_tick();
                if let Some(said) = said {
                    self.mission_note(client, said);
                }
            }
            Err(e) => self.mission_note(client, format!("Couldn't save it: {e}")),
        }
    }

    /// What mission control's line shows, while the mode is on and has no
    /// view of its own (src/autopilot/view.rs) to say it in instead.
    pub fn autopilot_summary(&self) -> Option<Summary> {
        let mode = &self.autopilot.mode;
        (settings::is_on("autopilot") && mode.on && !super::has_view()).then(|| Summary {
            failing: self.sources.main_failing(),
            ..self.autopilot.pilot.summary(mode, self.autopilot_away())
        })
    }

    /// Its own view in mission control (`view`), with UX 2.0 and the
    /// view_autopilot setting on: while the mode is on, or `keep` (a client
    /// has it in front). `landing` is the Landing view's items, whose
    /// entries say where each tab's worktree is in its landing queue; then
    /// today's landings of the mode's note whose tabs are closed. Built from
    /// what the server holds, reading no file.
    pub fn autopilot_view(
        &self,
        keep: bool,
        landing: &[crate::items::Item],
    ) -> Option<Vec<crate::items::Item>> {
        if !super::has_view() {
            return None;
        }
        let mode = &self.autopilot.mode;
        if !(mode.on || keep) {
            return None;
        }
        let rows = self.autopilot_rows(landing);
        let (summary, eta) = self.autopilot_heading(&rows);
        Some(self.autopilot_items(&summary, &eta, &rows))
    }

    /// The view's items from its rows and heading.
    fn autopilot_items(
        &self,
        summary: &Summary,
        eta: &str,
        rows: &[super::view::Row],
    ) -> Vec<crate::items::Item> {
        let mode = &self.autopilot.mode;
        let skipped: Vec<String> = mode.skipped_here().map(str::to_string).collect();
        let mut items = super::view::items(
            summary,
            eta,
            &mode.said,
            rows,
            &self.autopilot.pilot.next.queue,
            &self.autopilot.pilot.next.held,
            &skipped,
            &self.autopilot.pilot.next.plans,
        );
        let failures = self.autopilot.pilot.start_failures(mode, util::now_ms());
        super::view::say_failed_starts(&mut items, &failures);
        items
    }

    /// A row for each of the mode's tabs, then for each of today's
    /// landings of its note whose tab is closed.
    fn autopilot_rows(&self, landing: &[crate::items::Item]) -> Vec<super::view::Row> {
        let mode = &self.autopilot.mode;
        let marked = |opts: &std::collections::BTreeMap<String, String>, name: &str| {
            opts.get(name).is_some_and(|v| !v.is_empty() && v != "0")
        };
        let mut rows = Vec::new();
        let mut open = std::collections::HashSet::new();
        for model in self.sidebar_all_tabs() {
            let Some(tab) = self.tabs.get(&model.id) else {
                continue;
            };
            if !marked(&tab.opts, OPENED) {
                continue;
            }
            // A tab made yours stays on the board of its own note alone.
            let other_note = tab.opts.get(NOTE).is_some_and(|n| *n != mode.note_id);
            if marked(&tab.opts, YOURS) && other_note {
                continue;
            }
            if let Some(since) = tab
                .opts
                .get(OPENED)
                .and_then(|v| v.trim().parse::<f64>().ok())
            {
                open.insert(format!("{}@{since}", model.id.0));
            }
            let id = model.id.to_string();
            let landing = landing.iter().find_map(|i| {
                let l = i.data.get("landing")?;
                (l.get("tab").and_then(serde_json::Value::as_str) == Some(id.as_str()))
                    .then(|| l.clone())
            });
            let (started, ended) = super::view::times_of_tab(&tab.opts);
            rows.push(super::view::Row {
                tab: model.id,
                index: model.index,
                item: tab
                    .opts
                    .get(ITEM)
                    .cloned()
                    .unwrap_or_else(|| model.name.clone()),
                state: model.state,
                was_in: marked(&tab.opts, IN),
                loop_skill: model.loop_skill.clone(),
                phase: model.phase.clone(),
                landing,
                left: marked(&tab.opts, LEFT) || model.autopilot_landed,
                autoland: model.autopilot_autoland,
                closed: None,
                in_note: !other_note,
                started,
                ended,
                phases: self
                    .tab_panes(model.id)
                    .into_iter()
                    .filter_map(|p| self.agents.get(p))
                    .find(|a| !a.phases.is_empty())
                    .map(|a| a.phases.clone())
                    .unwrap_or_default(),
                now: util::now_secs(),
            });
        }
        let now = util::now_ms();
        for l in &mode.landed {
            if l.note != mode.note_id || !mode::same_day(l.at, now) || open.contains(&l.id) {
                continue;
            }
            let tab =
                l.id.split_once('@')
                    .and_then(|(t, _)| t.parse().ok())
                    .map_or(TabId(0), TabId);
            let (started, ended) = super::view::times_of_closed(l);
            rows.push(super::view::Row {
                tab,
                index: 0,
                item: l.item.clone(),
                state: TabState::Idle,
                was_in: true,
                loop_skill: None,
                phase: None,
                landing: None,
                left: true,
                autoland: false,
                closed: Some(l.clone()),
                in_note: true,
                started,
                ended,
                phases: Vec::new(),
                now: util::now_secs(),
            });
        }
        rows
    }

    /// What the heading says, and when the note's ready items should be
    /// done (empty while stopped or with Settings' "Estimated finish" off).
    fn autopilot_heading(&self, rows: &[super::view::Row]) -> (Summary, String) {
        let mode = &self.autopilot.mode;
        let summary = Summary {
            failing: self.sources.main_failing(),
            ..self.autopilot.pilot.summary(mode, self.autopilot_away())
        };
        // Paused, away, past today's spend limit or at the weekly limit, it
        // opens nothing for now.
        let held_off = summary.paused
            || summary.away
            || (summary.spend_limit > 0.0 && summary.spent >= summary.spend_limit)
            || (summary.weekly_limit > 0
                && summary
                    .weekly
                    .is_some_and(|(used, _)| used >= f64::from(summary.weekly_limit)));
        // Stopped, it starts nothing more, so it has nothing to estimate.
        let eta = if settings::is_on("autopilot_eta") && mode.on {
            super::view::eta(
                rows,
                &mode.landed,
                mode.slots,
                mode.cap,
                held_off,
                &self.autopilot.pilot.next.left,
                &jiff::Zoned::now(),
            )
        } else {
            String::new()
        };
        (summary, eta)
    }

    /// `autopilot-board`: autopilot's progress for the Slack relay's Home
    /// tab (board 81), as JSON, from what the view is built from: whether
    /// it is on, paused (by you, and since when), away or past its spend
    /// limit, what it last said, its note's title, the spend and limit,
    /// when the ready items should be done, how many items stand in each of
    /// the board's columns, and its tabs in flight (working, waiting on you
    /// or landing), each with its tab and column (never its item, a note's words), and how
    /// many landed today.
    pub fn autopilot_board(&mut self) -> serde_json::Value {
        use serde_json::json;
        let mode = self.autopilot.mode.clone();
        if !settings::is_on("autopilot") || !mode.on {
            return json!({ "on": false });
        }
        let landing = self
            .landing_view(false)
            .map(|d| d.items)
            .unwrap_or_default();
        let rows = self.autopilot_rows(&landing);
        let (summary, eta) = self.autopilot_heading(&rows);
        let items = self.autopilot_items(&summary, &eta, &rows);
        let columns: Vec<serde_json::Value> = super::view::COLUMNS
            .iter()
            .map(|c| {
                let n = items
                    .iter()
                    .filter(|i| i.group.as_deref() == Some(*c))
                    .count();
                json!([c, n])
            })
            .collect();
        let mut flying: Vec<(usize, &super::view::Row)> = rows
            .iter()
            .filter(|r| r.closed.is_none())
            .map(|r| (super::view::column(r, &super::view::stages(r)), r))
            .filter(|(c, _)| (super::view::WORKING..=super::view::LANDING).contains(c))
            .collect();
        // Waiting on you first, then landing, then working, as board 81 lists them.
        flying.sort_by_key(|(c, r)| ([3, 0, 1][*c - super::view::WORKING], r.index));
        let tabs: Vec<serde_json::Value> = flying
            .into_iter()
            .map(|(c, r)| {
                json!({
                    "tab": r.tab.to_string(),
                    "index": r.index,
                    "column": super::view::COLUMNS[c],
                    "started": r.started,
                })
            })
            .collect();
        let limited = summary.spend_limit > 0.0 && summary.spent >= summary.spend_limit;
        // At the weekly limit, as its last reading says, for the Home tab to
        // say when the week resets.
        let weekly = summary
            .weekly
            .filter(|(used, resets_at)| {
                summary.weekly_limit > 0
                    && *used >= f64::from(summary.weekly_limit)
                    && *resets_at > util::now_secs()
            })
            .map(|(used, resets_at)| {
                json!({ "limit": summary.weekly_limit, "used": used, "resetsAt": resets_at })
            });
        // The Landed column keeps a landed tab open from another day; the
        // Home tab counts today's alone.
        let now = util::now_ms();
        let landed_today = rows
            .iter()
            .filter(|r| super::view::column(r, &super::view::stages(r)) == super::view::LANDED)
            .filter(|r| r.ended.is_some_and(|e| mode::same_day(e * 1000, now)))
            .count();
        json!({
            "on": true,
            "note": note_title(&mode.note),
            "paused": summary.paused,
            "pausedAt": (summary.paused && mode.paused_at > 0).then_some(mode.paused_at),
            "away": summary.away,
            "limited": limited,
            "weekly": weekly,
            "said": mode.said,
            "slots": mode.slots,
            "spent": summary.spent,
            "limit": summary.spend_limit,
            "eta": eta,
            "columns": columns,
            "landedToday": landed_today,
            "tabs": tabs,
        })
    }

    /// `autopilot-pause on|off`: paused or resumed, as its menu's Pause
    /// and Resume do, from the Slack relay's Home tab: what it says of it.
    pub fn autopilot_pause(&mut self, pause: bool) -> Result<String, String> {
        if !Mode::read(&paths::state_dir()).on {
            return Err("Autopilot is off".into());
        }
        let act = if pause { Act::Pause } else { Act::Resume };
        let mut said = None;
        Mode::update(&paths::state_dir(), |m| said = line::apply(&act, m))
            .map_err(|e| format!("Couldn't save it: {e}"))?;
        self.autopilot.pilot.reset();
        self.autopilot_tick();
        Ok(said.unwrap_or_default())
    }

    // ── The check ───────────────────────────────────────────────────────────

    /// Whether autopilot holds off opening tabs: with no client attached,
    /// which a tab needs to open in, and while you're away
    /// (`Server::away_now`) only with the autopilot_while_away setting off.
    fn autopilot_away(&self) -> bool {
        self.clients.is_empty() || (self.away_now() && !settings::is_on("autopilot_while_away"))
    }

    /// Every tick: the mode read again, its tabs marked, and the next item
    /// read or opened when it is time.
    pub fn autopilot_tick(&mut self) {
        self.autopilot_refused();
        self.autopilot_failed();
        let state = paths::state_dir();
        let mut mode = Mode::read(&state);
        // Hands-free counts only while its setting allows it.
        mode.hands_free &= settings::is_on("autopilot_handsfree");
        self.autopilot_notices(&mode);
        let on = settings::is_on("autopilot");
        let repo = mode
            .repo
            .canonicalize()
            .unwrap_or_else(|_| mode.repo.clone());
        let mine = if mode.set_up() {
            self.autopilot_mine(&repo)
        } else {
            Vec::new()
        };
        // A tab opened before it had its plan looked for (an older build's).
        for one in &mine {
            self.find_tab_plan(one.tab);
        }
        let notes = worktree_from_note::notes_dir();
        let mtime = (mode.set_up() && !mode.note.is_empty())
            .then(|| modified_ms(&notes.join(&mode.note)))
            .flatten();
        let here = (on && mode.on && !mode.note_id.is_empty() && mtime.is_some())
            .then(|| self.autopilot_id_at(&notes.join(&mode.note)))
            .flatten();
        let away = self.autopilot_away();
        let counts = self.autopilot.pilot.counts;
        let close = settings::cached("autopilot_close")
            .and_then(|v| v.as_str().and_then(super::pilot::close_after));
        let check = self.autopilot.pilot.check(
            &mode,
            on,
            close,
            &mine,
            away,
            util::now_ms(),
            mtime,
            here.as_deref(),
        );
        let changed = self.autopilot.mode != mode || self.autopilot.pilot.counts != counts;
        let lost_said = mode.said.starts_with("Can't find the note");
        // Nothing left is said only with none of its tabs open; one said
        // while some were (an older build's) goes.
        let empty_said_early =
            mode.said.starts_with("Nothing left") && self.autopilot.pilot.counts.1 > 0;
        self.autopilot.mode = mode;
        self.autopilot_marks(&check.marks, &mine);
        if (check.found_again && lost_said) || empty_said_early {
            self.autopilot_say("");
        }
        if let Some(said) = check.said {
            self.autopilot_say(&said);
        }
        if let Some(keep) = check.keep {
            self.autopilot_keep(keep, notes.clone());
        }
        if let Some(job) = check.read_next {
            self.autopilot_read_next(job, notes.clone());
        }
        if let Some(job) = check.open {
            self.autopilot_look(job, notes);
        }
        if changed {
            self.autopilot_redraw();
        }
    }

    /// The ID the note at `file` holds, read again only once the file
    /// changes, since a check runs every tick: its last bytes, a small read.
    fn autopilot_id_at(&mut self, file: &Path) -> Option<String> {
        let stamp = stamp_of(file)?;
        if let Some((seen, at, id)) = &self.autopilot.seen_id
            && seen == file
            && *at == stamp
        {
            return id.clone();
        }
        let id = crate::notes::id::of_file(file);
        self.autopilot.seen_id = Some((file.to_path_buf(), stamp, id.clone()));
        id
    }

    /// Following the note by its ID, off the task.
    fn autopilot_keep(&mut self, keep: Keep, notes: PathBuf) {
        let followed = keep.follows();
        if tokio::runtime::Handle::try_current().is_err() {
            self.autopilot
                .pilot
                .kept(&Kept::default(), followed, util::now_ms());
            return;
        }
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let kept = keep.run(&notes);
            let _ = tx.send(Event::Autopilot(AutopilotEvent::Kept { kept, followed }));
        });
    }

    /// What following the note found, written back: the ID it was given
    /// (its skipped items from before IDs with it), the path it was found
    /// at, older tabs' notes' IDs, and what was said of it gone once it is
    /// found.
    fn autopilot_kept(&mut self, kept: Kept, followed: bool) {
        self.autopilot.pilot.kept(&kept, followed, util::now_ms());
        for claim in &kept.unplaced {
            eprintln!("autopilot: couldn't find the note of the claim \"{claim}\", left as it was");
        }
        for (tab, note_id) in &kept.tabs {
            let Some(t) = self.tabs.get_mut(tab) else {
                continue;
            };
            if !t.opts.contains_key(OPENED) || t.opts.contains_key(NOTE) {
                continue;
            }
            t.opts.insert(NOTE.into(), note_id.clone());
            self.dirty_tab(*tab);
        }
        let back = kept.found.is_some() || kept.gave_back;
        if kept.given.is_none() && !back {
            return;
        }
        let written = Mode::update(&paths::state_dir(), |m| {
            if let Some(given) = &kept.given
                && m.note == kept.note
                && m.note_id.is_empty()
            {
                m.note_id = given.clone();
                for x in m.skipped.iter_mut().filter(|x| x.note.is_empty()) {
                    x.note = given.clone();
                }
            }
            if let Some(found) = &kept.found
                && !m.note_id.is_empty()
                && m.note_id == kept.note_id
            {
                m.note = found.clone();
            }
            if back && m.said.starts_with("Can't find the note") {
                m.said.clear();
            }
        });
        if let Some(found) = &kept.found {
            eprintln!("autopilot: {} is now {found}", kept.note);
        }
        match written {
            Ok(mode) => {
                self.autopilot_notices(&mode);
                self.autopilot.mode = mode;
                self.autopilot.pilot.reset_next();
                self.autopilot_redraw();
            }
            Err(e) => eprintln!("autopilot: couldn't keep its note's ID: {e}"),
        }
    }

    /// The tabs it opened, as a check needs them, those made yours left out.
    fn autopilot_mine(&self, repo: &Path) -> Vec<Mine> {
        let opened = |opts: &std::collections::BTreeMap<String, String>| {
            if opts.contains_key(YOURS) {
                return None;
            }
            opts.get(OPENED)
                .and_then(|v| v.trim().parse::<f64>().ok())
                .filter(|n| *n != 0.0)
        };
        if !self.tabs.values().any(|t| opened(&t.opts).is_some()) {
            return Vec::new();
        }
        let models: std::collections::HashMap<TabId, _> = self
            .sidebar_all_tabs()
            .into_iter()
            .map(|t| (t.id, t))
            .collect();
        let mut out = Vec::new();
        for session in &self.sessions {
            for id in &session.tabs {
                let Some(tab) = self.tabs.get(id) else {
                    continue;
                };
                let Some(since) = opened(&tab.opts) else {
                    continue;
                };
                let model = models.get(id);
                let panes = tab.layout.panes();
                let state = self
                    .parked_state(&panes)
                    .or(model.map(|m| m.state))
                    .unwrap_or_default();
                // Where it works: its Claude's directory, else its panes'
                // (Notes, which M-e may have put beside it, runs in your home).
                let mut dirs: Vec<PathBuf> = model
                    .and_then(|m| m.claude.as_ref()?.cwd.as_ref().map(PathBuf::from))
                    .into_iter()
                    .chain(model.and_then(|m| m.path.clone()))
                    .collect();
                for pane in &panes {
                    let Some(slot) = self.panes.get(pane) else {
                        continue;
                    };
                    let marked = |name: &str| {
                        slot.opts
                            .get(name)
                            .is_some_and(|v| !v.is_empty() && v != "0")
                    };
                    if marked("@sidebar") || marked("@notes_pane") {
                        continue;
                    }
                    dirs.extend(slot.pane.cwd());
                }
                let marked = |name: &str| {
                    tab.opts
                        .get(name)
                        .is_some_and(|v| !v.is_empty() && v != "0")
                };
                let landed = tab
                    .opts
                    .get(LEFT)
                    .and_then(|v| v.trim().parse::<f64>().ok())
                    .filter(|n| *n != 0.0);
                // Its Claude's own pane, idle, in a main clone: whichever,
                // since the mode may have moved to another repository since
                // it opened the tab. Only Claude's pane counts when it has
                // one, since a shell beside it may be in the main clone
                // already while queue-and-clean still asks something.
                let claude_dirs: Vec<PathBuf> = match self.agents.leader(&panes) {
                    Some((p, _, _)) => self
                        .panes
                        .get(&p)
                        .and_then(|s| s.pane.cwd())
                        .into_iter()
                        .collect(),
                    None => dirs.clone(),
                };
                let home = state == TabState::Idle && in_main_clone(&claude_dirs);
                let everywhere = self.panes_with_parked(*id);
                let prompted_since = landed.is_some_and(|at| {
                    state != TabState::Idle
                        && everywhere
                            .iter()
                            .any(|p| self.agents.get(*p).is_some_and(|a| a.since as f64 > at))
                });
                let stand_in = panes.iter().any(|p| {
                    self.panes
                        .get(p)
                        .is_some_and(|s| s.opts.contains_key(PARKED_PANE))
                });
                out.push(Mine {
                    tab: *id,
                    since,
                    was_in: marked(IN),
                    item: tab.opts.get(ITEM).filter(|v| !v.is_empty()).cloned(),
                    note: tab.opts.get(NOTE).filter(|v| !v.is_empty()).cloned(),
                    tab_note: tab.opts.get("@tab_note").filter(|v| !v.is_empty()).cloned(),
                    state,
                    in_worktree: in_worktree_of(&dirs, repo),
                    in_main: in_main_clone(&dirs),
                    landed,
                    seen: marked(SEEN),
                    front: self.tab_in_front(*id),
                    stand_in,
                    home,
                    prompted_since,
                    land: tab.opts.get(LAND).is_some_and(|v| v == "1"),
                    hands_free: tab.opts.get(HANDS_FREE).is_some_and(|v| v == "1"),
                });
            }
        }
        out
    }

    /// While a tool stands in for a tab's pane (M-d's diff), the state of
    /// the pane parked behind it, background work included, as the tab's
    /// own would be.
    fn parked_state(&self, panes: &[PaneId]) -> Option<TabState> {
        let parked: PaneId = panes
            .iter()
            .find_map(|p| self.panes.get(p)?.opts.get(PARKED_PANE)?.parse().ok())?;
        self.panes.get(&parked)?;
        let work = self.sidebar.panes.get(&parked).map(|r| &r.work);
        let agent = self.agents.state(parked);
        Some(
            if agent == Some(State::Waiting) || work.is_some_and(|w| w.api_error.is_some()) {
                TabState::Waiting
            } else if agent == Some(State::Working) {
                TabState::Working
            } else if work.is_some_and(|w| w.count > 0 || w.scheduled.is_some()) {
                TabState::Background
            } else if agent == Some(State::Done) {
                TabState::Done
            } else {
                TabState::Idle
            },
        )
    }

    /// Its tabs marked as they reach their worktree, land (each landing kept
    /// for the view) and are seen, made yours as they leave the mode, and
    /// closed once landed and left, one closed without your having seen it
    /// kept for the notifications.
    fn autopilot_marks(&mut self, marks: &[Mark], mine: &[Mine]) {
        for mark in marks {
            let tab = match *mark {
                Mark::Note(t, _)
                | Mark::In(t)
                | Mark::Land(t, _)
                | Mark::HandsFree(t, _)
                | Mark::Left(t)
                | Mark::Landed(t, _)
                | Mark::Seen(t) => t,
                Mark::Close(t) => {
                    if let Some(m) = mine.iter().find(|m| m.tab == t && !m.seen) {
                        self.autopilot_closed_unseen(m);
                    }
                    self.autopilot_close_tab(t);
                    eprintln!("autopilot: closed {t}, landed");
                    continue;
                }
            };
            let Some(t) = self.tabs.get_mut(&tab) else {
                continue;
            };
            match mark {
                Mark::Note(_, note_id) => {
                    t.opts.insert(NOTE.into(), note_id.clone());
                }
                Mark::In(_) => {
                    t.opts.insert(IN.into(), "1".into());
                }
                Mark::Land(_, true) => {
                    t.opts.insert(LAND.into(), "1".into());
                }
                Mark::Land(_, false) => {
                    t.opts.remove(LAND);
                }
                Mark::HandsFree(_, true) => {
                    t.opts.insert(HANDS_FREE.into(), "1".into());
                }
                Mark::HandsFree(_, false) => {
                    t.opts.remove(HANDS_FREE);
                }
                Mark::Landed(_, at) => {
                    t.opts.insert(LEFT.into(), at.to_string());
                }
                Mark::Seen(_) => {
                    t.opts.insert(SEEN.into(), "1".into());
                }
                Mark::Left(_) | Mark::Close(_) => {
                    for option in WORKING {
                        t.opts.remove(option);
                    }
                    t.opts.insert(YOURS.into(), "1".into());
                }
            }
            self.dirty_tab(tab);
            if let Mark::Landed(..) = mark
                && let Some(m) = mine.iter().find(|m| m.tab == tab)
            {
                self.autopilot_keep_landed(m);
            }
        }
    }

    /// The commit `tab` landed, short: the one its landing marked it with,
    /// else the main clone's as its Claude's pane (else any of its panes)
    /// finds it there.
    fn autopilot_hash_of(&self, tab: TabId) -> String {
        // The commit its landing pushed, which queue-and-clean marks it with,
        // since a target branch other than the main clone's moves no HEAD.
        let pushed = self.tabs.get(&tab).and_then(|t| {
            [PUSHED, AUTOLAND]
                .iter()
                .find_map(|k| t.opts.get(*k).filter(|v| !v.trim().is_empty()))
        });
        if let Some(hash) = pushed {
            return hash.trim().chars().take(7).collect();
        }
        let panes = self
            .tabs
            .get(&tab)
            .map(|t| t.layout.panes())
            .unwrap_or_default();
        let dir = self
            .agents
            .leader(&panes)
            .map(|(p, _, _)| p)
            .into_iter()
            .chain(panes.iter().copied())
            .find_map(|p| self.panes.get(&p)?.pane.cwd());
        dir.and_then(|d| head_of(&d))
            .map(|h| h.chars().take(7).collect())
            .unwrap_or_default()
    }

    /// A tab just landed, kept in the mode's file for the view's Landed,
    /// which lists it once the tab is closed too.
    fn autopilot_keep_landed(&mut self, m: &Mine) {
        let landed = mode::Landed {
            id: format!("{}@{}", m.tab.0, m.since),
            item: m.item.clone().unwrap_or_else(|| self.tab_label(m.tab)),
            note: m
                .note
                .clone()
                .unwrap_or_else(|| self.autopilot.mode.note_id.clone()),
            hash: self.autopilot_hash_of(m.tab),
            // When its landing pushed, so closing the tab keeps the board's
            // timer where it stopped; now for a tab marked before there was
            // such a mark.
            at: self
                .tabs
                .get(&m.tab)
                .and_then(|t| t.opts.get(LANDED_AT))
                .and_then(|v| v.trim().parse::<i64>().ok())
                .filter(|s| *s != 0)
                .map_or_else(util::now_ms, |s| s * 1000),
        };
        // Only a landing that pushed counts for its issue (a tab merely
        // back in a main clone may have landed nothing), by the commit it
        // pushed; asked about unless it landed without asking.
        let opts = self
            .tabs
            .get(&m.tab)
            .map(|t| t.opts.clone())
            .unwrap_or_default();
        let set = |k: &str| opts.get(k).map(|v| v.trim()).filter(|v| !v.is_empty());
        // Landed without asking only as the tab was marked to (`@autopilot_land`,
        // which nothing in a pane can set), not by its own say alone.
        let without_asking = set(AUTOLAND).is_some() && set(LAND) == Some("1");
        // Whole, as the landing marked it, for GitHub's check; cut short only
        // where it is shown.
        let hash: Option<String> = set(AUTOLAND).or(set(PUSHED)).map(str::to_string);
        let opened_at = set(OPENED)
            .and_then(|v| v.parse::<f64>().ok())
            .map_or(0, |v| v as i64);
        let issue = set(ISSUE).map(str::to_string).zip(hash);
        let mut fresh = false;
        let written = Mode::update(&paths::state_dir(), |mode| {
            if mode.landed.iter().any(|l| l.id == landed.id) {
                return;
            }
            fresh = true;
            // A landed issue stays out of the queue whatever becomes of its
            // comment and closing (declined, failed, nobody there to ask),
            // kept among the skipped items, which can take it back.
            if issue.is_some() {
                let done = mode::Skipped {
                    note: landed.note.clone(),
                    text: landed.item.clone(),
                };
                if !mode.skipped.contains(&done) {
                    mode.skipped.push(done);
                }
            }
            // An item put next is next no more once it lands.
            mode.next
                .retain(|x| x.note != landed.note || x.text != landed.item);
            mode.landed.push(landed);
        });
        match written {
            Ok(mode) => self.autopilot.mode = mode,
            Err(e) => eprintln!("autopilot: couldn't keep the tab that landed: {e}"),
        }
        if let Some((issue, hash)) = issue.filter(|_| fresh) {
            let short: String = hash.chars().take(7).collect();
            // The landing's comment names the claim it releases, so the
            // release itself says nothing more.
            let id = super::issues::own_claim_id()
                .ok()
                .flatten()
                .unwrap_or_default();
            let comment = super::issues::landed_comment(&short, &id);
            let question = format!("Comment on {issue} that it landed in {short}, and close it?");
            self.autopilot_write_back(
                &issue,
                comment,
                Some((hash, opened_at)),
                without_asking,
                question,
            );
        }
    }

    /// Whether autopilot takes GitHub issues now: chosen, with its settings
    /// and GitHub's on.
    fn autopilot_writes_issues(&self) -> bool {
        let mode = Mode::read(&paths::state_dir());
        mode.sources.issues() && mode.issues_on()
    }

    /// A comment on the GitHub issue `issue` names (`<owner>/<repo>#<n>`),
    /// closing it as done once GitHub has the commit `landed` names, off the
    /// task, only while autopilot takes issues: at once for a tab that lands
    /// without asking, else once `question` is answered yes in the client
    /// used last (none attached, nothing is written, and it says so). A
    /// failure is said in the notifications.
    fn autopilot_write_back(
        &mut self,
        issue: &str,
        comment: String,
        landed: Option<(String, i64)>,
        without_asking: bool,
        question: String,
    ) {
        let Some((repo, number)) = super::issues::parse_reference(issue) else {
            return;
        };
        // Written only while autopilot takes GitHub issues, checked again
        // once a question is answered.
        if !self.autopilot_writes_issues() {
            return;
        }
        let named = issue.to_string();
        let close = landed.is_some();
        // Off the task, its failure written into what the mode says, which
        // the next check shows in the notifications, whoever is attached by
        // then.
        let write = move |server: &mut Server| {
            let issue = named;
            if !server.autopilot_writes_issues() {
                return;
            }
            if tokio::runtime::Handle::try_current().is_err() {
                return;
            }
            tokio::spawn(async move {
                let done = super::issues::write_back(
                    &repo,
                    number,
                    &comment,
                    close,
                    landed.as_ref().map(|(h, at)| (h.as_str(), *at)),
                    || {
                        let mode = Mode::read(&paths::state_dir());
                        mode.sources.issues() && mode.issues_on()
                    },
                )
                .await;
                // Closed as landed, the claim's label comes off; the
                // landing's comment named it. Left open, the claim holds.
                let done = match done {
                    Ok(true) if close => match super::issues::own_claim_id() {
                        Ok(Some(id)) => super::issues::release(&repo, number, &id, None)
                            .await
                            .map(|_| ()),
                        Ok(None) => Ok(()),
                        Err(e) => Err(e),
                    },
                    done => done.map(|_| ()),
                };
                let said = done.err().map(|e| format!("{WRITE_FAILED}{issue}: {e}"));
                let _ = tokio::task::spawn_blocking(move || {
                    let state = paths::state_dir();
                    super::issues::forget(&state);
                    if let Some(said) = said {
                        eprintln!("autopilot: {said}");
                        let _ = Mode::update(&state, |m| {
                            m.said = if m.said.is_empty() {
                                said
                            } else {
                                format!("{}. {said}", m.said)
                            };
                        });
                    }
                })
                .await;
            });
        };
        // Stopped, it writes nothing unasked: the tabs still working finish,
        // and what they land is asked about.
        if without_asking && Mode::read(&paths::state_dir()).on {
            write(self);
            return;
        }
        let Some(client) = self.latest_client() else {
            self.autopilot_say(&format!(
                "Left {issue} as it was: nobody was attached to ask before writing on it"
            ));
            return;
        };
        self.ask(
            client,
            crate::ui::Confirm::new(question),
            move |server, _, yes| {
                if yes {
                    write(server);
                }
            },
        );
    }

    /// This computer's claim on the GitHub issue `issue` names released, off
    /// the task, `why` said in a comment. The tab's own claim, so it is
    /// released whatever autopilot's sources are now, while Settings allow
    /// autopilot's issue writes (autopilot, its GitHub issues and GitHub). A
    /// failure is said in the notifications.
    fn autopilot_release(&mut self, issue: &str, why: &'static str) {
        let Some((repo, number)) = super::issues::parse_reference(issue) else {
            return;
        };
        // The settings that allow issue writes at all, not the sources chosen.
        let allowed = ["autopilot", "autopilot_github_issues", "github"]
            .iter()
            .all(|k| settings::is_on(k));
        if !allowed || tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let named = issue.to_string();
        tokio::spawn(async move {
            let released = async {
                let Some(id) = super::issues::own_claim_id()? else {
                    return anyhow::Ok(false);
                };
                super::issues::release(&repo, number, &id, Some(why)).await
            }
            .await;
            let _ = tokio::task::spawn_blocking(move || {
                let state = paths::state_dir();
                match released {
                    Ok(true) => super::issues::forget(&state),
                    Ok(false) => {}
                    Err(e) => {
                        let said = format!("{WRITE_FAILED}{named}: {e}");
                        eprintln!("autopilot: {said}");
                        let _ = Mode::update(&state, |m| {
                            m.said = if m.said.is_empty() {
                                said
                            } else {
                                format!("{}. {said}", m.said)
                            };
                        });
                    }
                }
            })
            .await;
        });
    }

    /// A tab about to close: one of autopilot's on a GitHub issue releases
    /// its claim there, saying so, unless autopilot's own landing pushed it,
    /// which keeps the claim until autopilot closes the issue, so the landed
    /// work isn't taken again elsewhere while the issue stays open.
    pub fn autopilot_tab_closing(&mut self, tab: TabId) {
        let Some(t) = self.tabs.get(&tab) else {
            return;
        };
        let set = |k: &str| t.opts.get(k).is_some_and(|v| !v.trim().is_empty());
        if !set(OPENED) {
            return;
        }
        let Some(issue) = t.opts.get(ISSUE).filter(|v| !v.trim().is_empty()).cloned() else {
            return;
        };
        // Pushed by autopilot's landing: kept until autopilot closes the
        // issue. Back in its main clone without such a push (a PR's merge),
        // there is nothing autopilot writes back, so it lets go.
        if set(PUSHED) || set(AUTOLAND) {
            return;
        }
        let why = if set(LEFT) {
            "its tab closed"
        } else {
            "its tab closed before it landed"
        };
        self.autopilot_release(&issue, why);
    }

    /// A landed tab about to close unseen, kept in its file for the
    /// notifications: "Closed <item>: landed <short hash>", the hash the
    /// main clone's commit as its Claude's pane finds it there.
    fn autopilot_closed_unseen(&mut self, m: &Mine) {
        let hash = self.autopilot_hash_of(m.tab);
        let item = m.item.clone().unwrap_or_else(|| self.tab_label(m.tab));
        let id = format!("{}@{}", m.tab.0, m.since);
        let number = self
            .sidebar_all_tabs()
            .iter()
            .find(|t| t.id == m.tab)
            .map(|t| t.index);
        let kept = id.clone();
        let written = Mode::update(&paths::state_dir(), |mode| {
            if mode.closed.iter().any(|c| c.id == id) {
                return;
            }
            mode.closed.push(mode::Closed {
                id,
                item,
                hash,
                at: util::now_ms(),
            });
            let over = mode.closed.len().saturating_sub(mode::CLOSED_KEPT);
            mode.closed.drain(..over);
        });
        match written {
            Ok(mode) => {
                self.autopilot_notices(&mode);
                self.autopilot.mode = mode;
                let words =
                    number.map_or("its tab closed".to_string(), |n| format!("tab {n} closed"));
                self.led_to("landed", &kept, None, &words);
            }
            Err(e) => eprintln!("autopilot: couldn't keep the tab it closed: {e}"),
        }
    }

    /// The landed tabs it closed unseen, put away: the one with this id, or
    /// every one.
    pub fn autopilot_dismiss_closed(&mut self, id: Option<&str>) {
        self.notices_dismissed("landed", id);
        match Mode::update(&paths::state_dir(), |m| match id {
            Some(id) => m.closed.retain(|c| c.id != id),
            None => m.closed.clear(),
        }) {
            Ok(mode) => {
                self.autopilot_notices(&mode);
                self.autopilot.mode = mode;
            }
            Err(e) => eprintln!("autopilot: couldn't save it: {e}"),
        }
    }

    /// A landed tab closed: with Notes moved beside it (M-e), only its own
    /// panes, so Notes, left alone, goes back to its tab and the tab closes
    /// (`notes_orphan`); else the tab.
    fn autopilot_close_tab(&mut self, tab: TabId) {
        let Some(t) = self.tabs.get(&tab) else {
            return;
        };
        let panes = t.layout.panes();
        let notes = |p: &PaneId| {
            self.panes
                .get(p)
                .and_then(|s| s.opts.get("@notes_pane"))
                .is_some_and(|v| !v.is_empty() && v != "0")
        };
        if panes.iter().any(notes) {
            let own: Vec<PaneId> = panes.iter().copied().filter(|p| !notes(p)).collect();
            for p in own {
                self.kill_pane(p);
            }
        } else {
            self.kill_tab(tab);
        }
    }

    /// Each of its tabs whose wtn refused the item (`REFUSED`): the item
    /// skipped in its note, why said, and the tab closed.
    fn autopilot_refused(&mut self) {
        type Refused = (TabId, String, String, Option<String>, Option<String>);
        let refused: Vec<Refused> = self
            .tabs
            .iter()
            .filter(|(_, t)| t.opts.contains_key(OPENED) && !t.opts.contains_key(YOURS))
            .filter_map(|(id, t)| {
                let why = t.opts.get(REFUSED).filter(|v| !v.is_empty())?;
                let item = t.opts.get(ITEM).filter(|v| !v.is_empty())?;
                let issue = t.opts.get(ISSUE).filter(|v| !v.is_empty()).cloned();
                Some((
                    *id,
                    item.clone(),
                    why.clone(),
                    t.opts.get(NOTE).cloned(),
                    issue,
                ))
            })
            .collect();
        for (tab, item, why, note, issue) in refused {
            let written = Mode::update(&paths::state_dir(), |m| {
                let skipped = mode::Skipped {
                    note: note
                        .clone()
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| m.note_id.clone()),
                    text: item.clone(),
                };
                if !m.skipped.contains(&skipped) {
                    m.skipped.push(skipped);
                }
            });
            match written {
                Ok(mode) => self.autopilot.mode = mode,
                Err(e) => {
                    eprintln!("autopilot: couldn't skip the refused item: {e}");
                    continue;
                }
            }
            self.autopilot_close_tab(tab);
            let duplicate = why.starts_with("Duplicate of");
            if duplicate {
                self.autopilot_say(&format!("Skipped {item}: {why}"));
            } else {
                self.autopilot_say(&format!("Skipped {item}: {REFUSED_SAID}, {why}"));
            }
            eprintln!("autopilot: closed {tab}, wtn refused its item");
            if let Some(issue) = issue.filter(|_| duplicate) {
                let comment = super::issues::duplicate_comment();
                let question = format!("Comment on {issue} that autopilot left it open: {why}?");
                // Always asked: landing without asking says nothing of posting
                // about a duplicate, which pushes nothing.
                self.autopilot_write_back(&issue, comment, None, false, question);
            }
        }
    }

    /// Each of its tabs whose wtn stopped before making its worktree (`FAILED`)
    /// and didn't refuse the item: the tab closed, so its slot is free and
    /// the item is opened again on a later check, and the item skipped, why
    /// said, once it has failed `pilot::FAILS_SKIP` times in an hour.
    fn autopilot_failed(&mut self) {
        type Failed = (TabId, String, String, String, bool);
        let failed: Vec<Failed> = self
            .tabs
            .iter()
            .filter(|(_, t)| t.opts.contains_key(OPENED) && !t.opts.contains_key(YOURS))
            .filter(|(_, t)| t.opts.get(REFUSED).is_none_or(|v| v.is_empty()))
            .filter_map(|(id, t)| {
                let why = t.opts.get(FAILED).filter(|v| !v.is_empty())?;
                let item = t.opts.get(ITEM).filter(|v| !v.is_empty())?;
                let note = t
                    .opts
                    .get(NOTE)
                    .filter(|n| !n.is_empty())
                    .cloned()
                    .unwrap_or_else(|| self.autopilot.mode.note_id.clone());
                // An issue's failure counts toward its skip once wtn has
                // claimed it, a failure of this computer's own after that.
                let issue = t.opts.get(ISSUE).is_some_and(|v| !v.is_empty())
                    && t.opts.get(CLAIMED).is_none_or(|v| v.is_empty());
                Some((*id, item.clone(), why.clone(), note, issue))
            })
            .collect();
        for (tab, item, why, note, issue) in failed {
            let key = (note.clone(), item.clone());
            // An issue's start fails on GitHub's say (a lookup failing, the
            // issue not standing for a moment), which passes: it is tried
            // again, never skipped for it.
            let skip = self
                .autopilot
                .pilot
                .start_failed(key, &why, util::now_ms(), !issue);
            self.autopilot_close_tab(tab);
            if !skip {
                eprintln!("autopilot: closed {tab}, its start failed: {why}");
                continue;
            }
            let written = Mode::update(&paths::state_dir(), |m| {
                let skipped = mode::Skipped {
                    note: note.clone(),
                    text: item.clone(),
                };
                if !m.skipped.contains(&skipped) {
                    m.skipped.push(skipped);
                }
            });
            match written {
                Ok(mode) => self.autopilot.mode = mode,
                Err(e) => eprintln!("autopilot: couldn't skip the item that failed to start: {e}"),
            }
            self.autopilot_say(&format!(
                "Skipped {item}: {FAILED_SAID} {} times in an hour, {why}",
                super::pilot::FAILS_SKIP
            ));
        }
    }

    /// The next item read again, off the task, for mission control's line.
    fn autopilot_read_next(&mut self, job: Job, notes: PathBuf) {
        if tokio::runtime::Handle::try_current().is_err() {
            self.autopilot.pilot.cancel();
            return;
        }
        let tx = self.tx.clone();
        let state = paths::state_dir();
        tokio::task::spawn_blocking(move || {
            let (queue, held, failed) = match job.items_and_held(&notes, &state) {
                Ok((queue, held)) => (queue, held, None),
                Err(e) => (
                    Vec::new(),
                    Vec::new(),
                    e.starts_with("GitHub issues:").then_some(e),
                ),
            };
            let left = autopilot_left(&job, &notes, &state, &queue, &held);
            // Every open item each held one waits on and where it stands,
            // from the note as it is now and the whole queue, before the
            // view's share of it is cut.
            let note = std::fs::read_to_string(notes.join(&job.note)).unwrap_or_default();
            let held = super::pilot::held_shown(&note, &queue, held);
            let queue: Vec<String> = queue
                .into_iter()
                .take(super::pilot::QUEUE_SHOWN)
                .map(|i| i.text)
                .collect();
            let text = queue.first().cloned().unwrap_or_default();
            let plans = job.plans(&notes, &state);
            let spent = if job.spend_limit > 0.0 {
                crate::usage::sessions::spent_today(&state, util::now_ms())
            } else {
                0.0
            };
            let weekly = if job.weekly_limit > 0 {
                crate::usage::sessions::weekly_used(&state, util::now_ms())
            } else {
                None
            };
            let _ = tx.send(Event::Autopilot(AutopilotEvent::Next {
                key: job.key,
                text,
                queue,
                held,
                plans,
                spent,
                weekly,
                left,
                failed,
            }));
        });
    }

    /// The next item to open looked for, off the task.
    fn autopilot_look(&mut self, job: Job, notes: PathBuf) {
        if tokio::runtime::Handle::try_current().is_err() {
            self.autopilot.pilot.cancel();
            return;
        }
        let state = paths::state_dir();
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let found = job.find(&notes, &state, util::now_ms());
            let _ = tx.send(Event::Autopilot(AutopilotEvent::Found {
                found,
                mtime: job.mtime,
            }));
        });
    }

    /// What the looks off the task found.
    pub fn autopilot_event(&mut self, event: AutopilotEvent) {
        match event {
            AutopilotEvent::Started {
                fields,
                checked,
                reply,
                asked,
            } => {
                let (fields, was) = *fields;
                let answer = if asked.elapsed() > START_LIMIT {
                    crate::mux::server::Reply::fail(
                        "Checking its settings took too long, so it didn't start",
                    )
                } else {
                    self.autopilot_started(&fields, &was, checked)
                };
                let _ = reply.send(crate::mux::server::Answer::Reply(answer));
            }
            AutopilotEvent::Next {
                key,
                text,
                queue,
                held,
                plans,
                spent,
                weekly,
                left,
                failed,
            } => {
                // A good read takes back what a failed one said.
                if failed.is_none() && self.autopilot.mode.said.contains(ISSUES_UNREAD) {
                    match Mode::update(&paths::state_dir(), |m| {
                        // Every read error said, whatever else is kept.
                        while m.said.contains(ISSUES_UNREAD) {
                            m.said = super::pilot::unsay(&m.said, ISSUES_UNREAD);
                        }
                    }) {
                        Ok(mode) => {
                            self.autopilot_notices(&mode);
                            self.autopilot.mode = mode;
                        }
                        Err(e) => eprintln!("autopilot: couldn't save what it said: {e}"),
                    }
                }
                if let Some(e) = failed {
                    let said = super::pilot::unread_said(&e);
                    if !self.autopilot.mode.said.contains(&said) {
                        self.autopilot_say(&said);
                    }
                    // The queue as last read stands: a failed read isn't an
                    // empty one, and is read again on the next check.
                    self.autopilot.pilot.read_failed(key, util::now_ms());
                    return;
                }
                let changed = self.autopilot.pilot.next.text != text
                    || self.autopilot.pilot.next.left != left
                    || self.autopilot.pilot.next.queue != queue
                    || self.autopilot.pilot.next.held != held
                    || self.autopilot.pilot.next.plans != plans
                    || self.autopilot.pilot.next.spent != spent
                    || self.autopilot.pilot.next.weekly != weekly;
                self.autopilot.pilot.next_read(key, text, util::now_ms());
                self.autopilot.pilot.next.queue = queue;
                self.autopilot.pilot.next.held = held;
                self.autopilot.pilot.next.plans = plans;
                self.autopilot.pilot.next.spent = spent;
                self.autopilot.pilot.next.weekly = weekly;
                self.autopilot.pilot.next.left = left;
                if changed {
                    self.autopilot_redraw();
                }
            }
            AutopilotEvent::Kept { kept, followed } => self.autopilot_kept(kept, followed),
            AutopilotEvent::Found { found, mtime } => {
                let mode = Mode::read(&paths::state_dir());
                let opened = self
                    .autopilot
                    .pilot
                    .opened(&mode, found, mtime, util::now_ms());
                if let Some(e) = opened.failed {
                    eprintln!("autopilot: {e}");
                }
                if let Some(said) = opened.said {
                    self.autopilot_say(&said);
                }
                if let Some(item) = opened.take {
                    self.autopilot_open(&mode, item);
                }
                self.autopilot_redraw();
            }
        }
    }

    /// A tab for the item found, unless the mode was paused or stopped
    /// meanwhile: in the session used last, in the background, marked as
    /// the mode's.
    fn autopilot_open(&mut self, mode: &Mode, item: Item) {
        if !settings::is_on("autopilot") || !mode.on || mode.paused || self.autopilot_away() {
            return;
        }
        let Some(client) = self.latest_client() else {
            return;
        };
        // An issue only while issues are still taken, and not skipped since
        // the look began (its landing recorded meanwhile, say).
        if let Some((repo, number)) = super::issues::issue_of(&item) {
            let reference = format!("{repo}#{number}: ");
            if !(mode.sources.issues() && mode.issues_on())
                || mode.skipped_here().any(|t| t.starts_with(&reference))
            {
                return;
            }
        }
        let opened = match super::issues::issue_of(&item) {
            Some((repo, number)) => self.open_issue_tab(
                client,
                &mode.repo,
                &format!("{repo}#{number}"),
                &item.text,
                &mode.target,
                crate::mux::server::Opened::Background,
            ),
            None => self.open_item_tab(
                client,
                &mode.repo,
                &mode.note,
                &item,
                &mode.target,
                crate::mux::server::Opened::Background,
            ),
        };
        let Some(tab) = opened else {
            return;
        };
        if let Some(t) = self.tabs.get_mut(&tab) {
            t.opts.insert(OPENED.into(), util::now_secs().to_string());
            t.opts.insert(ITEM.into(), item.text.clone());
            // Marked at once, so its wtn and its first question find it so.
            if mode.hands_free && settings::is_on("autopilot_handsfree") {
                t.opts.insert(HANDS_FREE.into(), "1".into());
            }
            if !mode.note_id.is_empty() {
                t.opts.insert(NOTE.into(), mode.note_id.clone());
            }
            // Marked as an issue's at once, so nothing takes it for a note's.
            if let Some((repo, number)) = super::issues::issue_of(&item) {
                t.opts.insert(ISSUE.into(), format!("{repo}#{number}"));
            }
        }
        self.dirty_tab(tab);
        // An issue's plan is its body: no note's plan is looked for.
        if super::issues::issue_of(&item).is_none() {
            self.find_tab_plan(tab);
        }
        self.autopilot
            .pilot
            .took(&mode.note_id, &item.text, util::now_ms());
        eprintln!(
            "autopilot: opened line {} of {} in {tab}",
            item.line, mode.note
        );
        if mode.said.contains("Nothing left to take") {
            // Only that: a failed write said after it stays until dismissed.
            match Mode::update(&paths::state_dir(), |m| {
                m.said = super::pilot::unsay(&m.said, "Nothing left to take")
            }) {
                Ok(mode) => {
                    self.autopilot_notices(&mode);
                    self.autopilot.mode = mode;
                }
                Err(e) => eprintln!("autopilot: couldn't save what it said: {e}"),
            }
        } else if mode.said.contains(super::pilot::WEEKLY) {
            // A tab opened, the weekly limit no longer holds it; a refusal
            // said before it stays.
            match Mode::update(&paths::state_dir(), |m| {
                m.said = super::pilot::unsay(&m.said, super::pilot::WEEKLY)
            }) {
                Ok(mode) => {
                    self.autopilot_notices(&mode);
                    self.autopilot.mode = mode;
                }
                Err(e) => eprintln!("autopilot: couldn't save what it said: {e}"),
            }
        }
    }

    // ── What it says ────────────────────────────────────────────────────────

    /// Its notification put away: what it said, forgotten.
    pub fn autopilot_dismiss(&mut self) {
        self.notices_dismissed("autopilot", None);
        self.autopilot_say("");
    }

    /// What it has to say, kept in its file for the notifications; empty
    /// for nothing.
    fn autopilot_say(&mut self, said: &str) {
        // A refusal not yet dismissed stays, what is said after it added, so
        // nothing said later hides why an item was skipped; empty, dismissed,
        // clears it too.
        // A read of GitHub issues failing takes back that nothing was left,
        // which it no longer knows.
        let without_empty = |was: &str| {
            if said.starts_with(ISSUES_UNREAD) {
                super::pilot::unsay(was, "Nothing left to take")
            } else {
                was.to_string()
            }
        };
        let keep = |was: &str| {
            let was = &without_empty(was);
            let kept = was.contains(REFUSED_SAID)
                || was.contains(FAILED_SAID)
                || was.contains(DUPLICATE_SAID)
                || was.contains(WRITE_FAILED);
            if said.is_empty() || !kept {
                said.to_string()
            } else if was.contains(said) {
                was.to_string()
            } else {
                format!("{was}. {said}")
            }
        };
        match Mode::update(&paths::state_dir(), |m| m.said = keep(&m.said)) {
            Ok(mode) => {
                self.autopilot_notices(&mode);
                self.autopilot.mode = mode;
            }
            Err(e) => eprintln!("autopilot: couldn't save what it said: {e}"),
        }
    }

    /// Its notifications, while the autopilot setting is on: what it said,
    /// and each landed tab it closed without your seeing it; and each
    /// landing its tabs made without asking, until dismissed, whatever the
    /// setting says now. With `autopilot_landing_notices` off, the closed
    /// tabs and the landings go straight into the history as dismissed and
    /// out of their files, never shown in the sidebar: the Autopilot view's
    /// Landed column keeps them from its own list.
    fn autopilot_notices(&mut self, mode: &Mode) {
        let on = settings::is_on("autopilot");
        let quiet = !settings::is_on("autopilot_landing_notices");
        let items = if on && !mode.said.is_empty() {
            vec![crate::items::Item::new("said", &mode.said).detail("autopilot")]
        } else {
            Vec::new()
        };
        self.set_notices("autopilot", items);
        let closed = if on {
            mode.closed
                .iter()
                .map(|c| {
                    let mut item = crate::items::Item::new(&c.id, c.title())
                        .detail("autopilot")
                        .time(c.at);
                    item.lines = vec![
                        "Closed by autopilot once it landed, without your seeing it (Settings' Close a worked note's landed tabs); its item's Done: line in the note says what landed".to_string(),
                    ];
                    item
                })
                .collect()
        } else {
            Vec::new()
        };
        if quiet && !closed.is_empty() {
            self.notices_put_away("landed", closed);
            if let Err(e) = Mode::update(&paths::state_dir(), |m| m.closed.clear()) {
                eprintln!("autopilot: couldn't put the closed tabs away: {e}");
            }
        } else {
            self.set_notices("landed", closed);
        }
        let autolanded: Vec<_> = super::landed::read(&paths::state_dir())
            .iter()
            .map(|l| {
                let mut item = crate::items::Item::new(&l.hash, l.title())
                    .detail("without asking")
                    .time(l.at);
                let at = util::local_at_ms(l.at).strftime("%-I:%M %p").to_string();
                item.lines = vec![format!(
                    "Landed on the default branch without asking, as autopilot's \"Land without asking\" allows, at {at}"
                )];
                item.data = serde_json::json!({ "tab": l.tab, "plan": l.plan });
                item
            })
            .collect();
        if quiet && !autolanded.is_empty() {
            self.notices_put_away("autolanded", autolanded);
            if let Err(e) = super::landed::dismiss(&paths::state_dir(), None) {
                eprintln!("autopilot: couldn't put the landings away: {e}");
            }
        } else {
            self.set_notices("autolanded", autolanded);
        }
    }

    /// The landings made without asking put away: the one with this hash,
    /// or every one.
    pub fn autopilot_dismiss_autolanded(&mut self, hash: Option<&str>) {
        self.notices_dismissed("autolanded", hash);
        if let Err(e) = super::landed::dismiss(&paths::state_dir(), hash) {
            eprintln!("autopilot: couldn't put the landing away: {e}");
        }
        let mode = self.autopilot.mode.clone();
        self.autopilot_notices(&mode);
    }

    /// The tab a landing without asking was made in, while it is open.
    pub fn autopilot_landed_tab(&self, item: &crate::items::Item) -> Option<TabId> {
        let tab: TabId = item.data.get("tab")?.as_str()?.parse().ok()?;
        self.tabs.contains_key(&tab).then_some(tab)
    }

    /// Mission control drawn again wherever it is open, for its line.
    fn autopilot_redraw(&mut self) {
        for c in self.clients.values_mut() {
            if c.overlays.find::<Mission>().is_some() {
                c.dirty = true;
            }
        }
    }
}

impl Server {
    /// Whether autopilot is on and works `note` (its path in the notes
    /// directory), for the "Work on this item" box's row putting an item
    /// next.
    pub fn autopilot_works(&self, note: &str) -> bool {
        let mode = &self.autopilot.mode;
        settings::is_on("autopilot") && mode.on && mode.sources.note() && mode.note == note
    }

    /// `autopilot-next <note> <line>`: the unticked item on that line of
    /// the note autopilot works put next in its queue, before the rest of
    /// its settled items, the last one put there first, until it lands. The
    /// note and its plan are read off the task; what it did, or why it
    /// couldn't, is said in `client`.
    pub fn autopilot_put_next(
        &mut self,
        client: ClientId,
        note: String,
        line: usize,
        expected: Option<String>,
    ) {
        if !self.autopilot_works(&note) {
            let why = format!("Autopilot isn't working {}.", note_title(&note));
            self.say(Some(client), &why);
            return;
        }
        let fallback = self.autopilot.mode.note_id.clone();
        let ready_only = self.autopilot.mode.ready_only;
        self.off_task(
            client,
            move || {
                next_item(
                    &worktree_from_note::notes_dir(),
                    &note,
                    line,
                    fallback,
                    ready_only,
                )
                .and_then(|found| match expected {
                    // The item on the line, as the box showed it: one moved
                    // there since is another item.
                    Some(text) if text != found.1 => Err(format!(
                        "The note changed: “{}” isn't on line {line} any more.",
                        crate::tabs::title::short_name(&text)
                    )),
                    _ => Ok(found),
                })
            },
            |server, client, found| {
                let said = found.and_then(|(id, text)| {
                    let written = Mode::update(&paths::state_dir(), |mode| {
                        // Put next, a skipped item is taken back.
                        mode.skipped.retain(|x| x.note != id || x.text != text);
                        mode.next.retain(|x| x.note != id || x.text != text);
                        mode.next.insert(
                            0,
                            mode::Skipped {
                                note: id.clone(),
                                text: text.clone(),
                            },
                        );
                    })
                    .map_err(|e| format!("Couldn't write autopilot's queue: {e}"))?;
                    server.autopilot.mode = written;
                    Ok(format!(
                        "Next in autopilot's queue: {}",
                        crate::tabs::title::short_name(&text)
                    ))
                });
                match said {
                    Ok(said) => server.say(Some(client), &said),
                    Err(why) => server.say(Some(client), &why),
                }
            },
        );
    }
}

/// The item on a line of a note autopilot works, as it is put next: its
/// note's ID (`fallback`, the mode's, when the note has none) and its first
/// line; refused when it is no unticked top-level item, it or its own plan
/// still needs a choice, or it has no plan while autopilot takes planned
/// items only.
fn next_item(
    notes: &Path,
    note: &str,
    line: usize,
    fallback: String,
    ready_only: bool,
) -> Result<(String, String), String> {
    let text = std::fs::read_to_string(notes.join(note))
        .map_err(|_| format!("No note {}.", note_title(note)))?;
    let item = worktree_from_note::item_at(&text, line)
        .map_err(|why| format!("Line {line} of {} {why}.", note_title(note)))?;
    let block = worktree_from_note::item_text(&text, line);
    if ready_only && !worktree_from_note::has_plan(notes, &block) {
        return Err(format!(
            "“{}” has no plan, and autopilot takes planned items only.",
            crate::tabs::title::short_name(&item.text)
        ));
    }
    let open = worktree_from_note::plan_open(notes, &block);
    if item.needs_choice || open {
        return Err(format!(
            "“{}” needs a choice first.",
            crate::tabs::title::short_name(&item.text)
        ));
    }
    let id = crate::notes::id::in_text(&text).unwrap_or(fallback);
    Ok((id, item.text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_item_put_next_is_refused_while_it_could_not_be_taken() {
        let dir = std::env::temp_dir().join(format!("sb-next-item-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Plan - Open.md"),
            "# Plan - Open\n\n## Decision needed\n\n- [ ] A: one\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("Work.md"),
            "# Work\n\n- [ ] Bare\n  - [ ] Child\n    Plan: [[Plan - Open]]\n- [ ] Open (chose A)\n  Plan: [[Plan - Open]]\n- [x] Done\n",
        )
        .unwrap();
        let next = |line, ready| next_item(&dir, "Work.md", line, "n-1".into(), ready);
        assert_eq!(next(3, false), Ok(("n-1".into(), "Bare".into())));
        assert!(next(3, true).unwrap_err().contains("planned items only"));
        assert!(next(6, false).unwrap_err().contains("needs a choice"));
        assert!(next(8, false).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn counts_and_dollars_as_typed() {
        assert_eq!(whole("", 3), Some(3), "empty keeps what it was");
        assert_eq!(whole("4", 3), Some(4));
        assert_eq!(whole("4.0", 3), Some(4));
        for bad in ["0", "-1", "2.5", "two"] {
            assert_eq!(whole(bad, 3), None, "{bad}");
        }
        assert_eq!(dollars(""), Some(0.0), "empty for none");
        assert_eq!(dollars("$20"), Some(20.0));
        assert_eq!(dollars("7.5"), Some(7.5));
        assert_eq!(dollars("-1"), None);
        assert_eq!(dollars("lots"), None);
        assert_eq!(percent(""), Some(0), "empty for none");
        assert_eq!(percent("80"), Some(80));
        assert_eq!(percent("80%"), Some(80));
        assert_eq!(percent(" 85 % "), Some(85));
        for bad in ["101", "-5", "82.5", "most"] {
            assert_eq!(percent(bad), None, "{bad}");
        }
    }
}
