//! Each tab as the sidebar and mission control see it, and where it goes
//! (lib/sidebar/model.js): its label and color, what its Claude is doing,
//! the work it runs in the background, its pin and reminder, and what
//! GitHub and the landing queue say of it, placed in one stage, which the
//! sidebar groups it under and mission control lists it under alike, in
//! order of what needs you most.
//!
//! The rules take the settings and the time they depend on as `Rules`, and
//! nothing here reads a file or the server, so each can be tested on tabs
//! made up for it; `serve` builds the tabs from the server's state.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::PathBuf;

use super::claude::{LoopRun, Scheduled, SessionRef, Turn};
use super::text::{self, Badge};
use crate::facts::{Checks, Landing, Method, PrState};
use crate::mux::{PaneId, TabId};
use crate::tabs::Rgb;

/// What a tab's Claude is doing, as the sidebar tells it: the agent state's
/// waiting, done and working, and two of its own. A Claude tab whose turn
/// ended with background tasks still running (a review loop, a CI watch) is
/// `Background`: Claude carries on when they end, so it is working, not
/// waiting for you. One whose turn an API error cut short is waiting, on the
/// error, until the conversation goes on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TabState {
    Waiting,
    Done,
    Working,
    Background,
    #[default]
    Idle,
}

impl TabState {
    /// Most in need of you first, the order agent-state.sh rolled a window's
    /// panes up by (model.js's `RANK`).
    pub fn rank(self) -> u8 {
        match self {
            TabState::Waiting => 0,
            TabState::Done => 1,
            TabState::Working | TabState::Background => 2,
            TabState::Idle => 3,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            TabState::Waiting => "waiting",
            TabState::Done => "done",
            TabState::Working => "working",
            TabState::Background => "background",
            TabState::Idle => "idle",
        }
    }

    /// Working, in the foreground or the background.
    pub fn busy(self) -> bool {
        matches!(self, TabState::Working | TabState::Background)
    }
}

/// The tab option a direct landing leaves, `<short hash> <branch> <clone
/// branch> <repo>`, the repository its main clone's git directory and the
/// clone branch what that main clone had checked out, which `switchboard
/// land-lock landed` sets. It counts only while the tab is in that main
/// clone on that branch (`landed_here`), so a tab gone on to other work, a
/// worktree, another repository or a new branch, however Claude started
/// there, isn't filed under Merged; a prompt given in the tab
/// (`Server::agent_hook`) or a worktree command taking the tab up
/// (`worktree::take_up_work`) takes it off.
pub const LANDED: &str = "@tab_landed";

/// A direct landing's mark read back: the short hash, the branch landed on,
/// the main clone's branch then and the repository's git directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LandedMark {
    pub hash: String,
    pub branch: String,
    pub clone_branch: String,
    pub repo: PathBuf,
}

/// `@tab_landed`'s value read back, the repository last since its path may
/// hold spaces.
pub fn landed_mark(value: &str) -> Option<LandedMark> {
    let mut words = value.trim().splitn(4, ' ');
    let (hash, branch, clone_branch) = (words.next()?, words.next()?, words.next()?);
    let repo = words.next()?.trim();
    let given = [hash, branch, clone_branch, repo];
    given.iter().all(|w| !w.is_empty()).then(|| LandedMark {
        hash: hash.to_string(),
        branch: branch.to_string(),
        clone_branch: clone_branch.to_string(),
        repo: PathBuf::from(repo),
    })
}

/// Whether a tab whose checkout git places (`tabs::git::locate`) is still
/// where its landing left it: the landed repository's main clone, not one
/// of its worktrees, on the branch it had then.
pub fn landed_here(mark: &LandedMark, place: &crate::tabs::git::GitPlace) -> bool {
    place.git_dir == place.common_dir
        && place.common_dir == mark.repo
        && place.branch == mark.clone_branch
}

/// Where a tab's pull request stands on its way in (prs.js's `tab.merge`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MergeState {
    Queued,
    Merged,
}

/// The loop a tab runs in the background: one of the loop skills' runs, or
/// a loop on a timer between its ticks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TabLoop {
    Run(LoopRun),
    Scheduled(Scheduled),
}

impl TabLoop {
    /// `review`, `ci`, `queue`, `deploy` or `scheduled`.
    pub fn kind(&self) -> &str {
        match self {
            TabLoop::Run(run) => run.kind,
            TabLoop::Scheduled(_) => "scheduled",
        }
    }
}

/// One tab, with all the sidebar and mission control show of it.
#[derive(Clone, Debug)]
pub struct TabModel {
    pub id: TabId,
    /// Its number in its session, from 1, as the status line shows it.
    pub index: usize,
    /// Its session's name.
    pub session: String,
    /// Whether a client shows its session.
    pub attached: bool,
    /// Whether it is its session's tab in front.
    pub current: bool,
    pub zoomed: bool,
    /// Its label without the PR in front (`@tab_label` after `#12 `).
    pub name: String,
    /// The name it was given, by hand or by a skill.
    pub own_name: Option<String>,
    pub pr: Option<u64>,
    /// The repository that PR is in, where its lookup found it (in a fork's
    /// clone, possibly the project upstream names); `None` when only its
    /// number is known, its directory's origin's then.
    pub pr_repo: Option<String>,
    /// Its directory's GitHub repository, origin's `owner/name`, as the
    /// lookups last read it; `None` outside one or before it is read.
    pub repo: Option<String>,
    /// The repository its directory is in, by its main clone's directory
    /// (`tabs::git::main_clone`), every worktree of one sharing it.
    pub repo_dir: Option<PathBuf>,
    /// In another repository than the one the sidebar is filtered to
    /// (`Side::repo`), so under Other repos whatever it is doing; only the
    /// sidebar's own model sets it, never mission control's.
    pub elsewhere: bool,
    pub color: Option<Rgb>,
    pub state: TabState,
    /// Pinned by hand (M-f) as needing you, until unpinned.
    pub pinned: bool,
    /// A reminder set on it (`@remind_at`, seconds, and `@remind_note`).
    pub remind_at: Option<i64>,
    pub remind_note: Option<String>,
    /// Opened by the mode that works a note continuously (`@autopilot`).
    pub autopilot: bool,
    /// Landed and back in the main clone, closing soon (`@autopilot_left`).
    pub autopilot_landed: bool,
    /// Landed without asking (`@autopilot_autoland`, the commit pushed).
    pub autopilot_autoland: bool,
    /// The one Notes, brought beside this tab's pane with M-e rather than
    /// in a tab of its own (src/notes/serve.rs).
    pub notes_here: bool,
    /// The Notes tab while Notes is beside another tab's pane: the pane
    /// standing in for it there (`switchboard notes away`).
    pub notes_away: bool,
    /// Which tab has Notes then, its number and name, for that tab's line.
    pub notes_owner: Option<(usize, String)>,
    /// Background tasks running.
    pub background: usize,
    /// The Monitor watches running, by description.
    pub monitors: Vec<String>,
    pub tab_loop: Option<TabLoop>,
    /// The tool call in a few words, as its hooks recorded it.
    pub activity: Option<String>,
    /// What it waits on: a permission, a question, an API error.
    pub wait: Option<String>,
    /// The same word for word, when its hook said it.
    pub ask: Option<crate::agent::Wait>,
    /// The tool call its Claude is on, while it works.
    pub doing: Option<crate::agent::Doing>,
    /// Its last turn in a sentence, as Haiku wrote it once the turn ended
    /// (src/mission/summary.rs).
    pub summary: Option<String>,
    /// The round its review loop is on, working or between rounds; 0 with
    /// none.
    pub round: usize,
    /// The loop skill its session runs, from when the skill was asked for.
    pub loop_skill: Option<String>,
    /// The loop's step, when it says (queue-and-clean's landing).
    pub phase: Option<String>,
    /// Since when it has been in its state, in seconds; for an idle tab,
    /// when it last did anything.
    pub since: Option<i64>,
    pub path: Option<PathBuf>,
    /// The program its pane runs, named for a tab with no Claude.
    pub command: Option<String>,
    pub panes: Vec<PaneId>,
    pub active: Option<PaneId>,
    /// The pane its Claude runs in, and its conversation.
    pub claude_pane: Option<PaneId>,
    pub claude: Option<SessionRef>,
    /// Its conversation's last turn, for mission control's cards and peeks.
    pub turn: Option<Turn>,
    pub merge: Option<MergeState>,
    /// Landed straight on a branch rather than through a PR, the short
    /// hash pushed and the branch (`@tab_landed`, which `land-lock landed`
    /// sets): filed under Merged as GitHub's merged PRs are.
    pub landed_on: Option<(String, String)>,
    /// Its place in the merge queue.
    pub position: Option<u32>,
    pub pr_state: Option<PrState>,
    pub checks: Option<Checks>,
    /// APPROVED, CHANGES_REQUESTED, REVIEW_REQUIRED.
    pub review: Option<String>,
    /// Lines added and deleted.
    pub size: Option<(u64, u64)>,
    pub landing: Option<Landing>,
}

impl TabModel {
    /// A tab with nothing known of it but its number and name, idle.
    pub fn new(id: TabId, index: usize, name: &str) -> TabModel {
        TabModel {
            id,
            index,
            session: String::new(),
            attached: false,
            current: false,
            zoomed: false,
            name: name.to_string(),
            own_name: None,
            pr: None,
            pr_repo: None,
            repo: None,
            repo_dir: None,
            elsewhere: false,
            color: None,
            state: TabState::Idle,
            pinned: false,
            remind_at: None,
            remind_note: None,
            autopilot: false,
            autopilot_landed: false,
            autopilot_autoland: false,
            notes_here: false,
            notes_away: false,
            notes_owner: None,
            background: 0,
            monitors: Vec::new(),
            tab_loop: None,
            activity: None,
            wait: None,
            ask: None,
            doing: None,
            summary: None,
            round: 0,
            loop_skill: None,
            phase: None,
            since: None,
            path: None,
            command: None,
            panes: Vec::new(),
            active: None,
            claude_pane: None,
            claude: None,
            turn: None,
            merge: None,
            landed_on: None,
            position: None,
            pr_state: None,
            checks: None,
            review: None,
            size: None,
            landing: None,
        }
    }
}

/// What the rules depend on that isn't the tab: the time, and the settings
/// that turn a kind of tab off (`reminders`, `build_failing`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rules {
    pub now_ms: i64,
    pub reminders: bool,
    pub build_failing: bool,
    /// UX 2.0: the sidebar opens with its tab count and the notifications,
    /// in place of the session's name with them pinned at the bottom.
    pub ux2: bool,
}

impl Rules {
    /// Now, with the settings as last read.
    pub fn now() -> Rules {
        Rules {
            now_ms: crate::util::now_ms(),
            reminders: crate::settings::is_on("reminders"),
            build_failing: crate::settings::is_on("build_failing"),
            ux2: crate::settings::is_on("ux2"),
        }
    }

    pub fn now_secs(&self) -> i64 {
        self.now_ms.div_euclid(1000)
    }
}

/// A reminder come due: it needs you as a pin does, until you go to the tab.
pub fn reminder_due(tab: &TabModel, r: &Rules) -> bool {
    r.reminders && tab.remind_at.is_some_and(|at| at * 1000 <= r.now_ms)
}

/// A reminder still to come: the tab waits under Reminders, with the time
/// left, while it isn't working, ahead of a failing build, since you set
/// the reminder to leave it until then.
pub fn reminder_later(tab: &TabModel, r: &Rules) -> bool {
    r.reminders && tab.remind_at.is_some_and(|at| at * 1000 > r.now_ms)
}

/// Whether a tab needs you: pinned, a reminder come due, or Claude waiting
/// on you or finished there.
pub fn needs_tab(tab: &TabModel, r: &Rules) -> bool {
    tab.pinned || reminder_due(tab, r) || matches!(tab.state, TabState::Waiting | TabState::Done)
}

/// Most in need of you first: by state, then waiting or finished longest,
/// then by number.
pub fn by_need(a: &TabModel, b: &TabModel) -> Ordering {
    a.state
        .rank()
        .cmp(&b.state.rank())
        .then_with(|| {
            if matches!(a.state, TabState::Waiting | TabState::Done) {
                a.since.unwrap_or(0).cmp(&b.since.unwrap_or(0))
            } else {
                Ordering::Equal
            }
        })
        .then(a.index.cmp(&b.index))
}

/// In Needs you, the tabs you pinned come first (you marked them yourself),
/// with the reminders come due.
fn pinned_first(a: &TabModel, b: &TabModel, r: &Rules) -> Ordering {
    let first = |t: &TabModel| t.pinned || reminder_due(t, r);
    first(b).cmp(&first(a)).then_with(|| by_need(a, b))
}

/// A merged PR's tab still at work on it: deploy-test waiting for the deploy
/// or recording its test, or Claude working under /deploy-test or the
/// /queue-and-clean that follows it after the merge. A direct landing has
/// no deploy to test: its queue-and-clean cleaning up stays under Merged.
pub fn testing_deploy(tab: &TabModel) -> bool {
    tab.merge == Some(MergeState::Merged)
        && tab.landed_on.is_none()
        && (tab.tab_loop.as_ref().is_some_and(|l| l.kind() == "deploy")
            || (tab.state.busy() && matches!(tab.loop_skill.as_deref(), Some("deploy" | "queue"))))
}

/// A tab whose only background work is watches (Monitor's, or a shell
/// command that only waits, `claude::WAITS`) is watching, not
/// working: Claude has nothing to do until one reports.
pub fn monitoring_only(tab: &TabModel) -> bool {
    tab.state == TabState::Background
        && !tab.monitors.is_empty()
        && tab.monitors.len() == tab.background
        && tab.tab_loop.is_none()
}

/// A self-paced or timed /loop between its ticks has nothing to do until its
/// next one, so it is in Loops rather than Working.
pub fn looping_only(tab: &TabModel) -> bool {
    tab.state == TabState::Background && matches!(tab.tab_loop, Some(TabLoop::Scheduled(_)))
}

/// An open PR, neither queued nor merged, with a check that failed.
pub fn build_failing(tab: &TabModel, r: &Rules) -> bool {
    r.build_failing
        && tab.pr_state == Some(PrState::Open)
        && tab.checks.is_some_and(|c| c.fail > 0)
        && tab.merge.is_none()
}

fn queue_order(a: &TabModel, b: &TabModel) -> Ordering {
    let place = |t: &TabModel| t.position.map_or(u64::MAX, u64::from);
    place(a).cmp(&place(b)).then_with(|| by_need(a, b))
}

fn landing_order(a: &TabModel, b: &TabModel) -> Ordering {
    let line = |t: &TabModel| t.landing.map_or(u32::MAX, |l| l.line);
    line(a).cmp(&line(b)).then_with(|| by_need(a, b))
}

/// Where a tab is, in this order: the sidebar's groups and mission control's
/// stages. Each loop skill at work (a review loop, a CI loop,
/// queue-and-clean) is a stage of its own rather than part of Working.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stage {
    Needs,
    Failing,
    Working,
    Monitoring,
    Review,
    Ci,
    Queue,
    Scheduled,
    Queued,
    Landing,
    Deploy,
    Merged,
    Idle,
    Later,
    Other,
    /// The tabs of every repository but the one the sidebar is filtered to.
    OtherRepos,
}

impl Stage {
    pub const ALL: [Stage; 16] = [
        Stage::Needs,
        Stage::Failing,
        Stage::Working,
        Stage::Monitoring,
        Stage::Review,
        Stage::Ci,
        Stage::Queue,
        Stage::Scheduled,
        Stage::Queued,
        Stage::Landing,
        Stage::Deploy,
        Stage::Merged,
        Stage::Idle,
        Stage::Later,
        Stage::Other,
        Stage::OtherRepos,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Stage::Needs => "needs",
            Stage::Failing => "failing",
            Stage::Working => "working",
            Stage::Monitoring => "monitoring",
            Stage::Review => "review",
            Stage::Ci => "ci",
            Stage::Queue => "queue",
            Stage::Scheduled => "scheduled",
            Stage::Queued => "queued",
            Stage::Landing => "landing",
            Stage::Deploy => "deploy",
            Stage::Merged => "merged",
            Stage::Idle => "idle",
            Stage::Later => "later",
            Stage::Other => "other",
            Stage::OtherRepos => "other-repos",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Stage::Needs => "Needs you",
            Stage::Failing => "Build failing",
            Stage::Working => "Working",
            Stage::Monitoring => "Monitoring",
            Stage::Review => "Review loop",
            Stage::Ci => "CI loop",
            Stage::Queue => "Queue-and-clean",
            Stage::Scheduled => "Loops",
            Stage::Queued => "In merge queue",
            Stage::Landing => "Landing",
            Stage::Deploy => "Testing Deploy",
            Stage::Merged => "Merged",
            Stage::Idle => "Idle",
            Stage::Later => "Reminders",
            Stage::Other => "Other",
            Stage::OtherRepos => "Other repos",
        }
    }

    /// The badge whose color its header wears.
    pub fn color(self) -> Badge {
        match self {
            Stage::Needs => Badge::Waiting,
            Stage::Failing => Badge::Failing,
            Stage::Working => Badge::Working,
            Stage::Monitoring => Badge::Monitor,
            Stage::Review => Badge::Review,
            Stage::Ci => Badge::Ci,
            Stage::Queue => Badge::Queue,
            Stage::Scheduled => Badge::Scheduled,
            Stage::Queued => Badge::Queued,
            Stage::Landing => Badge::Landing,
            Stage::Deploy => Badge::Deploy,
            Stage::Merged => Badge::Merged,
            Stage::Idle | Stage::Other | Stage::OtherRepos => Badge::Idle,
            Stage::Later => Badge::Later,
        }
    }

    fn order(self, a: &TabModel, b: &TabModel, r: &Rules) -> Ordering {
        match self {
            Stage::Needs => pinned_first(a, b, r),
            Stage::Queued => queue_order(a, b),
            Stage::Landing => landing_order(a, b),
            Stage::Later => a.remind_at.cmp(&b.remind_at),
            // By the stage each would have had, then as that stage orders.
            Stage::OtherRepos => {
                let (sa, sb) = (own_stage(a, r), own_stage(b, r));
                let rank = |s: Stage| Stage::ALL.iter().position(|x| *x == s);
                rank(sa).cmp(&rank(sb)).then_with(|| sa.order(a, b, r))
            }
            _ => by_need(a, b),
        }
    }

    fn of_loop(kind: &str) -> Stage {
        match kind {
            "review" => Stage::Review,
            "ci" => Stage::Ci,
            "queue" => Stage::Queue,
            "deploy" => Stage::Deploy,
            "scheduled" => Stage::Scheduled,
            _ => Stage::Working,
        }
    }
}

/// The one stage a tab is in: Other repos for one of another repository
/// than the sidebar's filter, whatever it is doing; else its own.
pub fn stage_of(tab: &TabModel, r: &Rules) -> Stage {
    if tab.elsewhere {
        return Stage::OtherRepos;
    }
    own_stage(tab, r)
}

/// The stage a tab is in by what it is doing: Needs you first, then its
/// PR's place, then the loop it is running, then its state, so a
/// queue-and-clean watching a queued PR shows the PR's place.
fn own_stage(tab: &TabModel, r: &Rules) -> Stage {
    if needs_tab(tab, r) {
        return Stage::Needs;
    }
    if tab.landing.is_some() {
        return Stage::Landing;
    }
    if testing_deploy(tab) {
        return Stage::Deploy;
    }
    match tab.merge {
        Some(MergeState::Queued) => return Stage::Queued,
        Some(MergeState::Merged) => return Stage::Merged,
        None => {}
    }
    // A reminder you set wins over what its checks say until it comes due.
    if !tab.state.busy() && reminder_later(tab, r) {
        return Stage::Later;
    }
    // A CI loop at work is the fix for a failing build, so it keeps its own
    // stage; once it stops, the tab is Build failing again.
    let ci_at_work = tab.state.busy()
        && (tab.tab_loop.as_ref().is_some_and(|l| l.kind() == "ci")
            || tab.loop_skill.as_deref() == Some("ci"));
    if build_failing(tab, r) && !ci_at_work {
        return Stage::Failing;
    }
    if monitoring_only(tab) {
        return Stage::Monitoring;
    }
    if tab.state == TabState::Background
        && let Some(l) = &tab.tab_loop
    {
        return Stage::of_loop(l.kind());
    }
    // Between a loop's rounds, Claude working in the foreground is the loop's too.
    if tab.state.busy()
        && let Some(skill) = &tab.loop_skill
    {
        return Stage::of_loop(skill);
    }
    if tab.state.busy() {
        Stage::Working
    } else if tab.claude.is_some() {
        Stage::Idle
    } else {
        Stage::Other
    }
}

/// Each tab's stage and place as `grouped` lays them out now, for holding
/// them still while you browse (`frozen_from`).
pub type Frozen = HashMap<TabId, (Stage, usize)>;

/// A stage and its tabs, in order.
#[derive(Clone, Debug)]
pub struct Staged<'a> {
    pub stage: Stage,
    pub tabs: Vec<&'a TabModel>,
}

/// The tabs in their stages, in the stages' order, empty ones left out:
/// the sidebar's groups and mission control's stages alike, so the two
/// never place a tab apart. `frozen` keeps each tab it knows in the stage
/// and place it had then, whatever its state since, so tabs hold still
/// while you browse them (going to a tab marks it seen, and states change
/// on their own); a tab opened since goes where its state puts it, after
/// them.
pub fn grouped<'a>(tabs: &'a [TabModel], frozen: Option<&Frozen>, r: &Rules) -> Vec<Staged<'a>> {
    let held = |t: &TabModel| frozen.and_then(|f| f.get(&t.id)).copied();
    Stage::ALL
        .iter()
        .filter_map(|&stage| {
            let mut in_it: Vec<&TabModel> = tabs
                .iter()
                .filter(|t| held(t).map_or_else(|| stage_of(t, r), |(s, _)| s) == stage)
                .collect();
            in_it.sort_by(|a, b| match (held(a), held(b)) {
                (Some((_, x)), Some((_, y))) => x.cmp(&y),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => stage.order(a, b, r),
            });
            (!in_it.is_empty()).then_some(Staged { stage, tabs: in_it })
        })
        .collect()
}

/// The tabs in the order the sidebar lists them, one list.
pub fn sidebar_order<'a>(
    tabs: &'a [TabModel],
    frozen: Option<&Frozen>,
    r: &Rules,
) -> Vec<&'a TabModel> {
    grouped(tabs, frozen, r)
        .into_iter()
        .flat_map(|g| g.tabs)
        .collect()
}

/// The tabs under Needs you, in the order the sidebar lists them: what M-a
/// walks.
pub fn needs_you<'a>(tabs: &'a [TabModel], r: &Rules) -> Vec<&'a TabModel> {
    let mut needs: Vec<&TabModel> = tabs
        .iter()
        .filter(|t| stage_of(t, r) == Stage::Needs)
        .collect();
    needs.sort_by(|a, b| pinned_first(a, b, r));
    needs
}

/// The tabs under Idle, in the order the sidebar lists them: what M-A
/// walks (model.js's `idleTabs`). Only the model can tell an idle tab from
/// one working in the background or looping.
pub fn idle_tabs<'a>(tabs: &'a [TabModel], r: &Rules) -> Vec<&'a TabModel> {
    let mut idle: Vec<&TabModel> = tabs
        .iter()
        .filter(|t| stage_of(t, r) == Stage::Idle)
        .collect();
    idle.sort_by(|a, b| Stage::Idle.order(a, b, r));
    idle
}

/// Each tab's stage and place as `grouped` lays them out now.
pub fn frozen_from(tabs: &[TabModel], r: &Rules) -> Frozen {
    grouped(tabs, None, r)
        .into_iter()
        .flat_map(|g| g.tabs.into_iter().map(move |t| (t.id, g.stage)))
        .enumerate()
        .map(|(at, (id, stage))| (id, (stage, at)))
        .collect()
}

/// The badge a tab wears (render.js's `badgeState`): pinned, whatever else
/// it is, since you pinned it yourself and its words still say what Claude
/// is doing; a reminder come due; else Claude's waiting or done; else its
/// landing, deploy test or merged PR (its work is over, and a tab can be
/// left marked working by a hook that came after its turn ended); for an
/// idle tab its PR's place; else a reminder still to come; else a failing
/// build; else its watch or loop; else its state.
pub fn badge_state(tab: &TabModel, r: &Rules) -> Badge {
    let needs = matches!(tab.state, TabState::Waiting | TabState::Done);
    let loop_badge = |kind: &str| Badge::of_loop(kind).unwrap_or(Badge::Working);
    if tab.pinned {
        Badge::Pinned
    } else if reminder_due(tab, r) {
        Badge::Reminder
    } else if needs {
        if tab.state == TabState::Waiting {
            Badge::Waiting
        } else {
            Badge::Done
        }
    } else if tab.landing.is_some() {
        Badge::Landing
    } else if testing_deploy(tab) {
        Badge::Deploy
    } else if tab.merge == Some(MergeState::Merged) {
        Badge::Merged
    } else if tab.state == TabState::Idle && tab.merge == Some(MergeState::Queued) {
        Badge::Queued
    } else if !tab.state.busy() && reminder_later(tab, r) {
        Badge::Later
    } else if tab.state == TabState::Idle && build_failing(tab, r) {
        Badge::Failing
    } else if monitoring_only(tab) {
        Badge::Monitor
    } else if let Some(l) = tab
        .tab_loop
        .as_ref()
        .filter(|_| tab.state == TabState::Background)
    {
        loop_badge(l.kind())
    } else if let Some(skill) = tab.loop_skill.as_deref().filter(|_| tab.state.busy()) {
        loop_badge(skill)
    } else {
        match tab.state {
            TabState::Working => Badge::Working,
            TabState::Background => Badge::Background,
            _ => Badge::Idle,
        }
    }
}

/// Where its PR is: `in merge queue at 3`, `merged`; or, landed straight on
/// a branch, `landed on main · 1a2b3c4`, `landed on main without asking`.
pub fn merge_words(tab: &TabModel) -> String {
    if let Some((hash, branch)) = &tab.landed_on {
        return if tab.autopilot_autoland {
            format!("landed on {branch} without asking")
        } else {
            format!("landed on {branch} · {hash}")
        };
    }
    match tab.merge {
        Some(MergeState::Queued) => match tab.position {
            Some(p) if p > 0 => format!("in merge queue at {p}"),
            _ => "in merge queue".into(),
        },
        Some(MergeState::Merged) => "merged".into(),
        None => String::new(),
    }
}

/// Its place in what lands: `landing`, `3rd in line to land`, or, its PR
/// in the merge queue, `merging`, `2nd in line to merge`.
pub fn landing_words(tab: &TabModel) -> String {
    match tab.landing {
        None => String::new(),
        Some(l) if l.method == Method::Pr && l.holding => "merging".into(),
        Some(l) if l.method == Method::Pr => {
            format!("{} in line to merge", text::ordinal(l.line))
        }
        Some(l) if l.holding => "landing".into(),
        Some(l) => format!("{} in line to land", text::ordinal(l.line)),
    }
}

fn reminder_words(tab: &TabModel, r: &Rules) -> Vec<String> {
    if reminder_due(tab, r) {
        vec![match &tab.remind_note {
            Some(note) if !note.is_empty() => format!("reminder: {note}"),
            _ => "reminder".into(),
        }]
    } else if reminder_later(tab, r) {
        let mut words = vec![format!(
            "remind {}",
            text::until(tab.remind_at.unwrap_or(0), r.now_ms)
        )];
        words.extend(tab.remind_note.clone().filter(|n| !n.is_empty()));
        words
    } else {
        Vec::new()
    }
}

fn joined(parts: impl IntoIterator<Item = String>) -> String {
    parts
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// What a tab is doing, in words: `waiting on you 4m · permission: Bash git
/// push`, `finished 2m ago`, `review loop · round 2`, with its pin, its
/// reminder and its PR's place.
pub fn state_words(tab: &TabModel, r: &Rules) -> String {
    let mut parts = Vec::new();
    if tab.pinned {
        parts.push("pinned".to_string());
    }
    parts.extend(reminder_words(tab, r));
    parts.push(state_only(tab, r));
    joined(parts)
}

pub fn state_only(tab: &TabModel, r: &Rules) -> String {
    // The Notes tab while Notes is beside another: where it went.
    if tab.notes_away {
        return match &tab.notes_owner {
            Some((index, name)) => format!("Notes is beside tab {index} · {name}"),
            None => "Notes isn't open".into(),
        };
    }
    let now = r.now_secs();
    let since = text::ago(tab.since, now);
    // A PR landing through the merge queue says its place once, as landing
    // words.
    let through_queue = tab.landing.is_some_and(|l| l.method == Method::Pr)
        && tab.merge == Some(MergeState::Queued);
    let merge = joined([
        landing_words(tab),
        if through_queue {
            String::new()
        } else {
            merge_words(tab)
        },
    ]);
    let and = |words: String| joined([words, merge.clone()]);
    let opt = |o: &Option<String>| o.clone().unwrap_or_default();
    match tab.state {
        TabState::Waiting => {
            let waiting = if since.is_empty() {
                "waiting on you".to_string()
            } else {
                format!("waiting on you {since}")
            };
            and(joined([waiting, opt(&tab.wait)]))
        }
        TabState::Done => {
            let finished = if since.is_empty() {
                "finished".to_string()
            } else {
                format!("finished {since} ago")
            };
            // The watches it left running, named as Monitoring names them.
            let watching = if tab.monitors.is_empty() {
                String::new()
            } else {
                joined(std::iter::once("monitoring".to_string()).chain(tab.monitors.clone()))
            };
            and(joined([finished, watching]))
        }
        TabState::Working => {
            let working = if since.is_empty() {
                "working".to_string()
            } else {
                format!("working {since}")
            };
            let skill = tab
                .loop_skill
                .as_deref()
                .and_then(Badge::of_loop)
                .map(|b| b.look().words.to_string())
                .unwrap_or_default();
            and(joined([
                working,
                skill,
                opt(&tab.phase),
                opt(&tab.activity),
            ]))
        }
        TabState::Background => {
            if let Some(TabLoop::Scheduled(s)) = &tab.tab_loop {
                let left = s.at - r.now_ms;
                // When first: a long prompt is cut at the line's end.
                let when = if left > 60_000 {
                    format!(
                        "next in {}",
                        text::ago(Some((r.now_ms - left).div_euclid(1000)), now)
                    )
                } else {
                    "next tick due".into()
                };
                let prompt = if s.prompt.is_empty() {
                    "loop".to_string()
                } else {
                    s.prompt.clone()
                };
                return and(format!("{when} · {prompt}"));
            }
            if let Some(TabLoop::Run(run)) = &tab.tab_loop {
                return and(match run.kind {
                    "queue" => format!(
                        "queue-and-clean · {}",
                        tab.phase.as_deref().unwrap_or("watching")
                    ),
                    "deploy" => "testing deploy".into(),
                    kind => format!(
                        "{} · round {}",
                        Badge::of_loop(kind).map_or(kind, |b| b.look().words),
                        run.round
                    ),
                });
            }
            if monitoring_only(tab) {
                let mut words = vec!["monitoring".to_string()];
                words.extend(tab.monitors.iter().cloned());
                return and(words.join(" · "));
            }
            let n = tab.background;
            and(format!(
                "working in background · {n} task{}",
                if n == 1 { "" } else { "s" }
            ))
        }
        TabState::Idle => {
            if !merge.is_empty() {
                return merge;
            }
            // A Claude tab's program goes without saying; any other's is named.
            let command = if tab.claude.is_some() {
                String::new()
            } else {
                opt(&tab.command)
            };
            let idle = if since.is_empty() {
                String::new()
            } else {
                format!("idle for {since}")
            };
            joined([idle, command])
        }
    }
}

/// A PR's checks, review and size in a few marks: `✓12 ✗1 ⏳3`, `approved`,
/// `+340 −80`, failing checks and changes asked for first.
pub fn pr_facts(tab: &TabModel) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(c) = tab.checks {
        let pending = crate::sidebar::text::pending_mark();
        let marks: Vec<String> = [("✓", c.pass), ("✗", c.fail), (pending, c.pending)]
            .iter()
            .filter(|(_, n)| *n > 0)
            .map(|(m, n)| format!("{m}{n}"))
            .collect();
        if !marks.is_empty() {
            out.push(marks.join(" "));
        }
    }
    if let Some(words) = tab.review.as_deref().and_then(|r| match r {
        "APPROVED" => Some("approved"),
        "CHANGES_REQUESTED" => Some("changes requested"),
        "REVIEW_REQUIRED" => Some("review required"),
        _ => None,
    }) {
        out.push(words.into());
    }
    if let Some((add, del)) = tab.size {
        out.push(format!("+{add} −{del}"));
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub const NOW_MS: i64 = 1_800_000_000_000;

    pub fn rules() -> Rules {
        Rules {
            now_ms: NOW_MS,
            reminders: true,
            build_failing: true,
            ux2: false,
        }
    }

    pub fn tab(n: u32, name: &str, state: TabState) -> TabModel {
        let mut t = TabModel::new(TabId(n), n as usize, name);
        t.state = state;
        t.claude = Some(SessionRef {
            session_id: "s".into(),
            ..SessionRef::default()
        });
        t
    }

    fn names<'a>(groups: &[Staged<'a>]) -> Vec<(&'static str, Vec<String>)> {
        groups
            .iter()
            .map(|g| {
                (
                    g.stage.title(),
                    g.tabs.iter().map(|t| t.name.clone()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn a_tab_whose_work_landed_straight_on_a_branch_is_under_merged_saying_where() {
        let r = rules();
        let mut landed = tab(1, "landed", TabState::Idle);
        landed.landed_on = Some(("abc1234".into(), "ux-2-0".into()));
        landed.merge = Some(MergeState::Merged);
        assert_eq!(stage_of(&landed, &r), Stage::Merged);
        assert_eq!(badge_state(&landed, &r), Badge::Merged);
        assert_eq!(state_words(&landed, &r), "landed on ux-2-0 · abc1234");
        // Without asking, as autopilot's "Land without asking" marks it.
        landed.autopilot = true;
        landed.autopilot_autoland = true;
        assert!(
            state_words(&landed, &r).ends_with("landed on ux-2-0 without asking"),
            "{}",
            state_words(&landed, &r)
        );
        // Its queue-and-clean cleaning up has no deploy to test.
        let mut cleaning = landed.clone();
        cleaning.state = TabState::Working;
        cleaning.loop_skill = Some("queue".into());
        assert!(!testing_deploy(&cleaning));
        assert_eq!(stage_of(&cleaning, &r), Stage::Merged);
        // Still landing, it is under Landing.
        cleaning.landing = Some(Landing {
            method: Method::Direct,
            holding: true,
            line: 1,
        });
        assert_eq!(stage_of(&cleaning, &r), Stage::Landing);
    }

    #[test]
    fn a_landing_mark_reads_back_its_hash_branch_and_repository_and_nothing_from_less() {
        let mark = landed_mark("abc1234 ux-2-0 main /code/my repo/.git").unwrap();
        assert_eq!(
            mark,
            LandedMark {
                hash: "abc1234".into(),
                branch: "ux-2-0".into(),
                clone_branch: "main".into(),
                repo: "/code/my repo/.git".into(),
            }
        );
        assert_eq!(landed_mark("abc1234 ux-2-0 main"), None);
        assert_eq!(landed_mark(" ux-2-0 main /r/.git"), None);
        assert_eq!(landed_mark(""), None);
        // Only in the landed repository's main clone, on its branch then.
        let place = |git_dir: &str, common_dir: &str, branch: &str| crate::tabs::git::GitPlace {
            branch: branch.into(),
            git_dir: git_dir.into(),
            common_dir: common_dir.into(),
        };
        let repo = "/code/my repo/.git";
        assert!(landed_here(&mark, &place(repo, repo, "main")));
        assert!(!landed_here(&mark, &place(repo, repo, "new-work")));
        let worktree = "/code/my repo/.git/worktrees/next";
        assert!(!landed_here(&mark, &place(worktree, repo, "main")));
        let other = "/code/other/.git";
        assert!(!landed_here(&mark, &place(other, other, "main")));
    }

    fn v(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_notes_tab_says_where_notes_went() {
        let r = rules();
        let mut notes = tab(1, "Notes", TabState::Idle);
        notes.notes_away = true;
        notes.notes_owner = Some((3, "api".into()));
        assert_eq!(state_words(&notes, &r), "Notes is beside tab 3 · api");
        notes.notes_owner = None;
        assert_eq!(state_words(&notes, &r), "Notes isn't open");
    }

    #[test]
    fn the_idle_tabs_are_the_idle_group_in_its_order() {
        let r = rules();
        let mut later = tab(1, "later", TabState::Idle);
        later.index = 7;
        let mut working = tab(2, "working", TabState::Working);
        working.index = 1;
        let mut bare = tab(3, "no claude", TabState::Idle);
        bare.claude = None;
        let mut first = tab(4, "first", TabState::Idle);
        first.index = 2;
        let tabs = vec![later, working, bare, first];
        let idle: Vec<&str> = idle_tabs(&tabs, &r)
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(idle, ["first", "later"]);
    }

    #[test]
    fn tabs_are_ordered_by_what_needs_you() {
        let r = rules();
        let mut tabs = vec![
            tab(1, "one", TabState::Working),
            tab(2, "two", TabState::Done),
            tab(3, "three", TabState::Waiting),
            tab(4, "four", TabState::Idle),
            tab(5, "five", TabState::Waiting),
        ];
        tabs[0].since = Some(100);
        tabs[1].since = Some(50);
        tabs[2].since = Some(80);
        tabs[4].since = Some(10);
        let order: Vec<usize> = sidebar_order(&tabs, None, &r)
            .iter()
            .map(|t| t.index)
            .collect();
        assert_eq!(order, vec![5, 3, 2, 1, 4]);
    }

    #[test]
    fn filtered_to_a_repository_the_others_tabs_are_under_other_repos_last() {
        let r = rules();
        let mut waiting_elsewhere = tab(1, "api waiting", TabState::Waiting);
        waiting_elsewhere.elsewhere = true;
        let mut idle_elsewhere = tab(2, "web idle", TabState::Idle);
        idle_elsewhere.elsewhere = true;
        let mine = tab(3, "mine working", TabState::Working);
        let mut other = tab(4, "my shell", TabState::Idle);
        other.claude = None;
        let tabs = vec![idle_elsewhere, waiting_elsewhere, mine, other];
        // After Other, ordered by the stage each would have had.
        assert_eq!(
            names(&grouped(&tabs, None, &r)),
            vec![
                ("Working", vec!["mine working".to_string()]),
                ("Other", vec!["my shell".to_string()]),
                (
                    "Other repos",
                    vec!["api waiting".to_string(), "web idle".to_string()]
                ),
            ]
        );
        // M-a and M-A pass them by.
        assert!(needs_you(&tabs, &r).is_empty());
        assert!(idle_tabs(&tabs, &r).is_empty());

        // Without the filter, every tab is where it was.
        let unfiltered: Vec<TabModel> = tabs
            .into_iter()
            .map(|mut t| {
                t.elsewhere = false;
                t
            })
            .collect();
        assert_eq!(
            names(&grouped(&unfiltered, None, &r)),
            vec![
                ("Needs you", vec!["api waiting".to_string()]),
                ("Working", vec!["mine working".to_string()]),
                ("Idle", vec!["web idle".to_string()]),
                ("Other", vec!["my shell".to_string()]),
            ]
        );
        assert_eq!(needs_you(&unfiltered, &r).len(), 1);
    }

    #[test]
    fn tabs_with_no_claude_are_other_under_idle_and_each_loop_its_own_group() {
        let r = rules();
        let mut notes = tab(1, "Notes", TabState::Idle);
        notes.claude = None;
        let resting = tab(2, "resting", TabState::Idle);
        let mut relay = tab(3, "Slack relay", TabState::Idle);
        relay.claude = None;
        let mut calendar = tab(4, "Calendar", TabState::Background);
        calendar.tab_loop = Some(TabLoop::Scheduled(Scheduled {
            prompt: "/today".into(),
            at: NOW_MS + 60_000,
        }));
        let mut busy = tab(5, "busy", TabState::Working);
        busy.tab_loop = calendar.tab_loop.clone();
        let mut reviewing = tab(6, "reviewing", TabState::Background);
        reviewing.tab_loop = Some(TabLoop::Run(LoopRun {
            kind: "review",
            round: 1,
        }));
        let tabs = vec![notes, resting, relay, calendar, busy, reviewing];
        assert_eq!(
            names(&grouped(&tabs, None, &r)),
            vec![
                ("Working", v(&["busy"])),
                ("Review loop", v(&["reviewing"])),
                ("Loops", v(&["Calendar"])),
                ("Idle", v(&["resting"])),
                ("Other", v(&["Notes", "Slack relay"])),
            ]
        );
    }

    #[test]
    fn a_frozen_layout_keeps_each_tab_in_its_group_and_place() {
        let r = rules();
        let mut before = vec![
            tab(1, "one", TabState::Idle),
            tab(2, "two", TabState::Done),
            tab(3, "three", TabState::Working),
            tab(4, "four", TabState::Idle),
        ];
        before[1].since = Some(10);
        let frozen = frozen_from(&before, &r);
        let mut after = before.clone();
        after[0].state = TabState::Waiting;
        after[0].since = Some(20);
        after[1].state = TabState::Idle;
        after[1].since = None;
        after[3].state = TabState::Working;
        let mut five = tab(5, "five", TabState::Waiting);
        five.since = Some(5);
        after.push(five);
        assert_eq!(
            names(&grouped(&after, Some(&frozen), &r)),
            vec![
                ("Needs you", v(&["two", "five"])),
                ("Working", v(&["three"])),
                ("Idle", v(&["one", "four"])),
            ]
        );
        assert_eq!(
            names(&grouped(&after, None, &r)),
            vec![
                ("Needs you", v(&["five", "one"])),
                ("Working", v(&["three", "four"])),
                ("Idle", v(&["two"])),
            ],
            "unfrozen, the live order"
        );
    }

    #[test]
    fn merge_queue_and_merged_tabs_have_groups_of_their_own() {
        let r = rules();
        let queued = |n, name: &str, state, position| {
            let mut t = tab(n, name, state);
            t.merge = Some(MergeState::Queued);
            t.position = Some(position);
            t
        };
        let mut landed = tab(4, "landed", TabState::Idle);
        landed.merge = Some(MergeState::Merged);
        let tabs = vec![
            tab(1, "idle one", TabState::Idle),
            queued(2, "queued third", TabState::Idle, 3),
            queued(3, "queued first", TabState::Working, 1),
            landed,
            queued(5, "asking", TabState::Waiting, 2),
        ];
        assert_eq!(
            names(&grouped(&tabs, None, &r)),
            vec![
                ("Needs you", v(&["asking"])),
                ("In merge queue", v(&["queued first", "queued third"])),
                ("Merged", v(&["landed"])),
                ("Idle", v(&["idle one"])),
            ]
        );
        assert_eq!(state_words(&tabs[1], &r), "in merge queue at 3");
        assert_eq!(state_words(&tabs[3], &r), "merged");
        assert_eq!(
            state_words(&tabs[4], &r),
            "waiting on you · in merge queue at 2"
        );
        assert_eq!(badge_state(&tabs[1], &r), Badge::Queued);
        assert_eq!(badge_state(&tabs[3], &r), Badge::Merged);
        assert_eq!(
            badge_state(&tabs[2], &r),
            Badge::Working,
            "a queued tab working keeps its spinner"
        );
    }

    #[test]
    fn landing_tabs_are_in_the_landing_queues_order() {
        let r = rules();
        let landing = |n, name: &str, holding, line| {
            let mut t = tab(n, name, TabState::Working);
            t.landing = Some(Landing {
                holding,
                line,
                method: Method::Direct,
            });
            t
        };
        let tabs = vec![
            tab(1, "elsewhere", TabState::Working),
            landing(2, "second", false, 3),
            landing(3, "lander", true, 1),
            landing(4, "first", false, 2),
        ];
        assert_eq!(
            names(&grouped(&tabs, None, &r)),
            vec![
                ("Working", v(&["elsewhere"])),
                ("Landing", v(&["lander", "first", "second"])),
            ]
        );
        assert_eq!(state_words(&tabs[1], &r), "working · 3rd in line to land");
        assert_eq!(state_words(&tabs[2], &r), "working · landing");
        assert_eq!(badge_state(&tabs[2], &r), Badge::Landing);

        // A tab whose PR is in the merge queue lands too, by its place.
        let mut queued = tab(5, "queued pr", TabState::Idle);
        queued.merge = Some(MergeState::Queued);
        queued.position = Some(2);
        queued.landing = Some(Landing {
            holding: false,
            line: 2,
            method: Method::Pr,
        });
        assert_eq!(state_words(&queued, &r), "2nd in line to merge");
        let mut all = tabs.clone();
        all.push(queued);
        assert_eq!(
            names(&grouped(&all, None, &r)),
            vec![
                ("Working", v(&["elsewhere"])),
                ("Landing", v(&["lander", "first", "queued pr", "second"])),
            ],
            "landing either way is one group"
        );
    }

    #[test]
    fn a_failing_build_is_a_group_of_its_own_and_a_ci_loop_at_work_on_it_another() {
        let r = rules();
        let pr = |n, name: &str, state, pr_state, fail| {
            let mut t = tab(n, name, state);
            t.pr = Some(10 + u64::from(n));
            t.pr_state = Some(pr_state);
            t.checks = Some(Checks {
                pass: 1,
                fail,
                pending: 0,
            });
            t
        };
        let mut queued = pr(5, "red queued", TabState::Idle, PrState::Open, 1);
        queued.merge = Some(MergeState::Queued);
        queued.position = Some(1);
        let mut ci = pr(6, "red ci loop", TabState::Background, PrState::Open, 1);
        ci.background = 1;
        ci.tab_loop = Some(TabLoop::Run(LoopRun {
            kind: "ci",
            round: 2,
        }));
        let tabs = vec![
            pr(1, "green", TabState::Idle, PrState::Open, 0),
            pr(7, "red closed", TabState::Idle, PrState::Closed, 1),
            pr(2, "red idle", TabState::Idle, PrState::Open, 1),
            pr(3, "red working", TabState::Working, PrState::Open, 2),
            pr(4, "red asking", TabState::Waiting, PrState::Open, 1),
            queued,
            ci,
        ];
        assert_eq!(
            names(&grouped(&tabs, None, &r)),
            vec![
                ("Needs you", v(&["red asking"])),
                ("Build failing", v(&["red working", "red idle"])),
                ("CI loop", v(&["red ci loop"])),
                ("In merge queue", v(&["red queued"])),
                ("Idle", v(&["green", "red closed"])),
            ]
        );
        let mut stopped = tabs[2].clone();
        stopped.loop_skill = Some("ci".into());
        assert_eq!(stage_of(&stopped, &r), Stage::Failing);
        assert_eq!(badge_state(&tabs[2], &r), Badge::Failing);
        assert_eq!(badge_state(&tabs[3], &r), Badge::Working);
        let off = Rules {
            build_failing: false,
            ..r
        };
        assert_eq!(stage_of(&tabs[2], &off), Stage::Idle);
    }

    #[test]
    fn background_work_monitors_and_loops_read_as_they_are() {
        let r = rules();
        let mut bg = tab(9, "review", TabState::Background);
        bg.background = 2;
        assert_eq!(state_words(&bg, &r), "working in background · 2 tasks");
        let done = tab(8, "done", TabState::Done);
        assert_eq!(
            names(&grouped(&[bg.clone(), done], None, &r)),
            vec![("Needs you", v(&["done"])), ("Working", v(&["review"]))]
        );
        let mut watcher = tab(7, "watcher", TabState::Background);
        watcher.background = 1;
        watcher.monitors = vec!["CI on #12".into()];
        assert_eq!(stage_of(&watcher, &r), Stage::Monitoring);
        assert_eq!(state_words(&watcher, &r), "monitoring · CI on #12");
        // A turn that ended over its watches, not yet seen: Needs you, the
        // watches named.
        let finished = TabModel {
            state: TabState::Done,
            ..watcher.clone()
        };
        assert_eq!(stage_of(&finished, &r), Stage::Needs);
        assert_eq!(badge_state(&finished, &r), Badge::Done);
        assert_eq!(
            state_words(&finished, &r),
            "finished · monitoring · CI on #12"
        );
        watcher.background = 2;
        assert_eq!(stage_of(&watcher, &r), Stage::Working, "other work as well");
        let mut looped = bg.clone();
        looped.tab_loop = Some(TabLoop::Run(LoopRun {
            kind: "ci",
            round: 3,
        }));
        assert_eq!(state_words(&looped, &r), "CI loop · round 3");
        looped.tab_loop = Some(TabLoop::Run(LoopRun {
            kind: "queue",
            round: 1,
        }));
        assert_eq!(state_words(&looped, &r), "queue-and-clean · watching");
        looped.phase = Some("waiting for the landing lock".into());
        assert_eq!(
            state_words(&looped, &r),
            "queue-and-clean · waiting for the landing lock"
        );
        looped.tab_loop = Some(TabLoop::Run(LoopRun {
            kind: "review",
            round: 2,
        }));
        assert_eq!(state_words(&looped, &r), "review loop · round 2");
        assert_eq!(badge_state(&looped, &r), Badge::Review);
        let mut working = tab(6, "w", TabState::Working);
        working.loop_skill = Some("queue".into());
        working.phase = Some("testing".into());
        working.activity = Some("Run the tests".into());
        assert_eq!(
            state_words(&working, &r),
            "working · queue-and-clean · testing · Run the tests"
        );
        let mut oncall = tab(5, "on call", TabState::Background);
        oncall.tab_loop = Some(TabLoop::Scheduled(Scheduled {
            prompt: "/oncall".into(),
            at: NOW_MS + 7 * 60_000 + 5_000,
        }));
        assert_eq!(stage_of(&oncall, &r), Stage::Scheduled);
        assert_eq!(state_words(&oncall, &r), "next in 7m · /oncall");
        // A queue-and-clean whose test run was moved to the background, its
        // turn over, is still queue-and-clean's.
        let mut moved = tab(7, "tests moved", TabState::Background);
        moved.background = 1;
        moved.loop_skill = Some("queue".into());
        assert_eq!(stage_of(&moved, &r), Stage::Queue);
    }

    #[test]
    fn a_tab_says_what_claude_does_and_waits_on() {
        let r = rules();
        let now = r.now_secs();
        let mut t = tab(1, "x", TabState::Working);
        t.since = Some(now - 120);
        t.activity = Some("Run the full test suite".into());
        assert_eq!(state_words(&t, &r), "working 2m · Run the full test suite");
        t.loop_skill = Some("review".into());
        assert_eq!(
            state_words(&t, &r),
            "working 2m · review loop · Run the full test suite"
        );
        assert_eq!(stage_of(&t, &r), Stage::Review);
        t.loop_skill = None;
        t.state = TabState::Waiting;
        t.wait = Some("permission: Bash git push".into());
        assert_eq!(
            state_words(&t, &r),
            "waiting on you 2m · permission: Bash git push"
        );
        let mut idle = tab(2, "Fix #12", TabState::Idle);
        idle.since = Some(now - 300);
        assert_eq!(state_words(&idle, &r), "idle for 5m");
        idle.claude = None;
        idle.command = Some("cat".into());
        assert_eq!(state_words(&idle, &r), "idle for 5m · cat");
    }

    #[test]
    fn pinned_tabs_lead_needs_you_whatever_claude_does() {
        let r = rules();
        let mut waiting = tab(1, "waiting long", TabState::Waiting);
        waiting.since = Some(10);
        let mut pinned = tab(2, "pinned", TabState::Idle);
        pinned.pinned = true;
        let mut done = tab(3, "done", TabState::Done);
        done.since = Some(20);
        let tabs = vec![waiting, pinned.clone(), done];
        assert_eq!(
            names(&grouped(&tabs, None, &r))[0],
            ("Needs you", v(&["pinned", "waiting long", "done"]))
        );
        for state in [TabState::Waiting, TabState::Done, TabState::Working] {
            let t = TabModel {
                state,
                ..pinned.clone()
            };
            assert_eq!(badge_state(&t, &r), Badge::Pinned);
        }
        let asking = TabModel {
            state: TabState::Waiting,
            ..pinned.clone()
        };
        assert_eq!(state_words(&asking, &r), "pinned · waiting on you");
        pinned.claude = None;
        pinned.pinned = false;
        assert_eq!(stage_of(&pinned, &r), Stage::Other);
    }

    #[test]
    fn a_reminder_waits_under_reminders_then_needs_you_once_due() {
        let r = rules();
        let now = r.now_secs();
        let mut t = tab(1, "x", TabState::Idle);
        t.claude = None;
        t.remind_at = Some(now + 90 * 60 + 30);
        t.remind_note = Some("check the deploy".into());
        assert_eq!(stage_of(&t, &r), Stage::Later);
        assert_eq!(stage_of(&t, &r), Stage::Later);
        assert_eq!(badge_state(&t, &r), Badge::Later);
        assert!(
            state_words(&t, &r).starts_with("remind in 1h 31m · check the deploy"),
            "{}",
            state_words(&t, &r)
        );
        let asking = TabModel {
            state: TabState::Waiting,
            ..t.clone()
        };
        assert_eq!(stage_of(&asking, &r), Stage::Needs);
        assert_eq!(badge_state(&asking, &r), Badge::Waiting);
        assert!(state_words(&asking, &r).ends_with("check the deploy · waiting on you"));
        assert_eq!(needs_you(std::slice::from_ref(&asking), &r).len(), 1);
        let busy = TabModel {
            state: TabState::Working,
            ..t.clone()
        };
        assert_eq!(stage_of(&busy, &r), Stage::Working);
        assert_eq!(badge_state(&busy, &r), Badge::Working);
        let merged = TabModel {
            merge: Some(MergeState::Merged),
            ..t.clone()
        };
        assert_eq!(stage_of(&merged, &r), Stage::Merged);
        // Come due, it needs you, with the pinned ones first.
        let mut due = t.clone();
        due.remind_at = Some(now - 5);
        let mut waiting = tab(99, "waiting", TabState::Waiting);
        waiting.since = Some(1);
        let tabs = vec![waiting, due.clone()];
        assert_eq!(
            names(&grouped(&tabs, None, &r)),
            vec![("Needs you", v(&["x", "waiting"]))]
        );
        assert_eq!(state_words(&due, &r), "reminder: check the deploy");
        assert_eq!(badge_state(&due, &r), Badge::Reminder);
        let off = Rules {
            reminders: false,
            ..r
        };
        assert_eq!(stage_of(&due, &off), Stage::Other);
    }

    #[test]
    fn a_reminder_to_come_wins_over_a_failing_build_until_due() {
        let r = rules();
        let now = r.now_secs();
        let mut t = tab(1, "x", TabState::Idle);
        t.pr_state = Some(PrState::Open);
        t.checks = Some(Checks {
            pass: 0,
            fail: 1,
            pending: 0,
        });
        assert_eq!(stage_of(&t, &r), Stage::Failing);
        t.remind_at = Some(now + 60 * 60);
        assert_eq!(stage_of(&t, &r), Stage::Later);
        assert_eq!(badge_state(&t, &r), Badge::Later);
        let mut due = t.clone();
        due.remind_at = Some(now - 5);
        assert_eq!(stage_of(&due, &r), Stage::Needs);
        assert_eq!(badge_state(&due, &r), Badge::Reminder);
        let busy = TabModel {
            state: TabState::Working,
            ..t.clone()
        };
        assert_eq!(stage_of(&busy, &r), Stage::Failing);
        let mut fixing = busy.clone();
        fixing.loop_skill = Some("ci".into());
        assert_eq!(stage_of(&fixing, &r), Stage::Ci);
        let off = Rules {
            reminders: false,
            ..r
        };
        assert_eq!(stage_of(&t, &off), Stage::Failing);
        assert_eq!(badge_state(&t, &off), Badge::Failing);
    }

    #[test]
    fn a_merged_tab_testing_its_deploy_is_before_merged() {
        let r = rules();
        let merged = |n, name: &str, state| {
            let mut t = tab(n, name, state);
            t.merge = Some(MergeState::Merged);
            t
        };
        let mut waiting_deploy = merged(1, "waiting on deploy", TabState::Background);
        waiting_deploy.tab_loop = Some(TabLoop::Run(LoopRun {
            kind: "deploy",
            round: 1,
        }));
        let mut recording = merged(2, "recording", TabState::Working);
        recording.loop_skill = Some("queue".into());
        let mut approve = merged(3, "approve the video", TabState::Waiting);
        approve.loop_skill = Some("deploy".into());
        let tabs = vec![
            waiting_deploy,
            recording,
            approve,
            merged(4, "done with it", TabState::Idle),
        ];
        assert_eq!(
            names(&grouped(&tabs, None, &r)),
            vec![
                ("Needs you", v(&["approve the video"])),
                ("Testing Deploy", v(&["waiting on deploy", "recording"])),
                ("Merged", v(&["done with it"])),
            ]
        );
        // Merged while marked working says merged; waiting still wins.
        let busy = merged(5, "b", TabState::Working);
        assert_eq!(badge_state(&busy, &r), Badge::Merged);
        let asking = merged(6, "a", TabState::Waiting);
        assert_eq!(badge_state(&asking, &r), Badge::Waiting);
    }

    #[test]
    fn every_state_a_tab_can_be_in_has_its_group() {
        let r = rules();
        let now = r.now_secs();
        let run = |kind| Some(TabLoop::Run(LoopRun { kind, round: 1 }));
        let at_work = |n, name: &str, state, skill: &str| {
            let mut t = tab(n, name, state);
            if state == TabState::Background {
                t.background = 1;
                t.tab_loop = run(match skill {
                    "review" => "review",
                    "ci" => "ci",
                    _ => "queue",
                });
            } else {
                t.loop_skill = Some(skill.into());
            }
            t
        };
        let mut pinned = tab(3, "pinned", TabState::Idle);
        pinned.pinned = true;
        let mut due = tab(4, "reminder due", TabState::Idle);
        due.remind_at = Some(now - 5);
        let mut later = tab(5, "reminder to come", TabState::Idle);
        later.remind_at = Some(now + 600);
        let landing = |n, name: &str, holding| {
            let mut t = tab(n, name, TabState::Working);
            t.landing = Some(Landing {
                holding,
                line: 1,
                method: Method::Direct,
            });
            t
        };
        let mut deploy = tab(8, "testing a deploy", TabState::Background);
        deploy.merge = Some(MergeState::Merged);
        deploy.tab_loop = run("deploy");
        let mut queued = tab(9, "in a merge queue", TabState::Idle);
        queued.merge = Some(MergeState::Queued);
        let mut merged = tab(10, "merged", TabState::Idle);
        merged.merge = Some(MergeState::Merged);
        let failing = |n, name: &str, state| {
            let mut t = tab(n, name, state);
            t.pr_state = Some(PrState::Open);
            t.checks = Some(Checks {
                pass: 0,
                fail: 1,
                pending: 0,
            });
            t
        };
        let mut fixing = failing(12, "a CI loop fixing it", TabState::Working);
        fixing.loop_skill = Some("ci".into());
        let mut watching = tab(13, "monitoring only", TabState::Background);
        watching.background = 1;
        watching.monitors = vec!["CI".into()];
        let mut timed = tab(20, "a /loop", TabState::Background);
        timed.tab_loop = Some(TabLoop::Scheduled(Scheduled {
            prompt: "/today".into(),
            at: NOW_MS + 60_000,
        }));
        let mut bare = tab(23, "no Claude", TabState::Idle);
        bare.claude = None;
        let tabs = vec![
            (tab(1, "waiting", TabState::Waiting), Stage::Needs),
            (tab(2, "done", TabState::Done), Stage::Needs),
            (pinned, Stage::Needs),
            (due, Stage::Needs),
            (later, Stage::Later),
            (landing(6, "landing", true), Stage::Landing),
            (landing(7, "waiting to land", false), Stage::Landing),
            (deploy, Stage::Deploy),
            (queued, Stage::Queued),
            (merged, Stage::Merged),
            (failing(11, "build failing", TabState::Idle), Stage::Failing),
            (fixing, Stage::Ci),
            (watching, Stage::Monitoring),
            (
                at_work(14, "review working", TabState::Working, "review"),
                Stage::Review,
            ),
            (
                at_work(
                    15,
                    "review in the background",
                    TabState::Background,
                    "review",
                ),
                Stage::Review,
            ),
            (
                at_work(16, "CI working", TabState::Working, "ci"),
                Stage::Ci,
            ),
            (
                at_work(17, "CI in the background", TabState::Background, "ci"),
                Stage::Ci,
            ),
            (
                at_work(18, "queue working", TabState::Working, "queue"),
                Stage::Queue,
            ),
            (
                at_work(19, "queue in the background", TabState::Background, "queue"),
                Stage::Queue,
            ),
            (timed, Stage::Scheduled),
            (tab(21, "working", TabState::Working), Stage::Working),
            (tab(22, "idle", TabState::Idle), Stage::Idle),
            (bare, Stage::Other),
        ];
        let models: Vec<TabModel> = tabs.iter().map(|(t, _)| t.clone()).collect();
        let shown: HashMap<String, Stage> = grouped(&models, None, &r)
            .into_iter()
            .flat_map(|g| g.tabs.into_iter().map(move |t| (t.name.clone(), g.stage)))
            .collect();
        for (t, stage) in &tabs {
            assert_eq!(shown.get(&t.name), Some(stage), "{}", t.name);
        }
        assert_eq!(Stage::Queue.title(), "Queue-and-clean");
        assert_eq!(Stage::Scheduled.title(), "Loops");
    }

    #[test]
    fn a_prs_facts_read_in_a_few_marks() {
        let mut t = tab(1, "x", TabState::Idle);
        t.checks = Some(Checks {
            pass: 12,
            fail: 1,
            pending: 3,
        });
        t.review = Some("APPROVED".into());
        t.size = Some((340, 80));
        assert_eq!(pr_facts(&t), vec!["✓12 ✗1 ⏳3", "approved", "+340 −80"]);
    }
}
