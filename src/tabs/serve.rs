//! Naming the tabs on the server: what tmux/window-names.sh did for every
//! window on every pane focus, done in the server task from `Names`, on a
//! focus, a tab switch and every tick. Each tab's label and color are kept
//! for the header, `switchboard list` and later parts (the sidebar, the
//! Slack relay) to read, as the window's `@tab_label` was, and the lookups a
//! directory needs run in background tasks that answer with
//! `Event::TabLookup`, so neither a focus nor a draw ever waits on the
//! network or starts a process.

use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::{paths, util};

use super::color::{self, Rgb};
use super::label::{self, PaneKind};
use super::names::{Answer, Ask};
use super::prs::{self, TabPr};
use super::{git, title};
use crate::integrations::github::Repo;
use crate::mux::TabId;
use crate::mux::server::{ClientId, Event, Server};
use crate::ui::{Menu, MenuOption};

/// Where the PRs and titles found are kept for the next server, as
/// window-pr.sh and ticket-title.sh kept theirs under `~/.cache`.
fn names_file() -> PathBuf {
    paths::cache_dir().join("tab-names.json")
}

/// What a tab shows, as last worked out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shown {
    /// The directory it is named from (the window's `@tab_path`).
    pub path: Option<PathBuf>,
    /// Its label with no name of its own and no PR.
    pub label: String,
    pub pr: Option<u64>,
    /// The repository its PR is in, where the lookup found it (in a fork's
    /// clone, possibly the project upstream names); `None` when only its
    /// number is known, its directory's origin's then.
    pub pr_repo: Option<Repo>,
    /// Its PR's title, once its lookup has answered.
    pub pr_title: Option<String>,
    /// Its label without a PR's title when only one can name it
    /// (`names::Facts::untitled`).
    pub untitled: Option<String>,
    /// The ticket its branch is for (`names::Facts::ticket`).
    pub ticket: Option<crate::views::ticket::Ticket>,
    /// The color it wears: its project's, or gray with the `accent_colors`
    /// setting off.
    pub color: Option<Rgb>,
    /// The repository its directory is in, by its main clone's directory
    /// (`git::main_clone`), for the sidebar's repository filter.
    pub repo_dir: Option<PathBuf>,
    /// The linked worktree the directory it is named from is in, by its top
    /// directory; `None` in a main clone or outside git.
    pub worktree: Option<PathBuf>,
    /// The branch checked out there, as git's own files say it; `None` on
    /// no branch or outside git.
    pub branch: Option<String>,
    /// Whether that branch is its repository's default branch, as git's own
    /// files say it (`git::is_default_branch`).
    pub on_default: bool,
    /// Whether the directory it was named from was removed under its pane
    /// (a worktree cleaned up), its name kept from before.
    pub gone: bool,
}

impl Server {
    /// Works out what a tab shows from the directory of the pane it is named
    /// after, starting any lookup that directory needs, and redraws the
    /// session when it changed.
    pub fn name_tab(&mut self, tab_id: TabId) {
        // The PRs and titles the last server found, before anything is
        // asked for again.
        if !self.tab_names.restored {
            self.tab_names.restored = true;
            if let Some(saved) = util::read_json(names_file()) {
                let names = &mut self.tab_names.names;
                names.restore(&saved, Instant::now(), util::now_secs());
            }
        }
        let Some(tab) = self.tabs.get(&tab_id) else {
            return;
        };
        let panes = tab.layout.panes();
        let kinds: Vec<(PaneKind, bool)> = panes
            .iter()
            .map(|p| {
                let opts = self.panes.get(p).map(|s| &s.opts);
                let marked = |name: &str| {
                    opts.and_then(|o| o.get(name))
                        .is_some_and(|v| !v.is_empty() && v != "0")
                };
                let kind = if marked("@sidebar") {
                    PaneKind::Sidebar
                } else if marked("@notes_pane") {
                    PaneKind::Notes
                } else {
                    PaneKind::Work
                };
                (kind, *p == tab.active)
            })
            .collect();
        let said = label::naming_pane(&kinds).and_then(|i| self.pane_dir_as_said(panes[i]));
        // A directory that can't be read (the pane's program just ended)
        // leaves the tab as it was, and so does one removed under the pane
        // once the tab was named from it (a worktree queue-and-clean cleaned
        // up, before the pane restarts in the main clone): the tab reads as
        // it did, its color and PR too, rather than as the ancestor left,
        // while its path (`@tab_path`, which the diff and others work in)
        // becomes that ancestor, a directory that exists. Arriving in the
        // main clone after that, it keeps the name (`keep_cleaned_name`).
        let Some(said) = said else {
            return;
        };
        let Some(dir) = util::nearest_existing(&said) else {
            return;
        };
        if dir != said
            && let Some(shown) = self.tab_names.shown.get_mut(&tab_id)
        {
            shown.path = Some(dir);
            shown.gone = true;
            return;
        }
        self.keep_cleaned_name(tab_id, &dir);
        let (facts, asks) = self.tab_names.names.facts(&dir, Instant::now());
        for ask in asks {
            self.start_lookup(ask);
        }
        let switches = self.tab_names.names.last_switches();
        let tint = switches
            .apart
            .then(|| self.own_tint(tab_id, facts.tint))
            .flatten();
        let place = git::locate(&dir);
        let shown = Shown {
            label: facts.label,
            pr: facts.pr,
            pr_repo: facts.pr_repo,
            pr_title: facts.pr_title,
            untitled: facts.untitled,
            ticket: facts.ticket,
            color: Some(match tint {
                _ if !switches.colors => color::GRAY,
                Some(tint) => color::tab_color(tint, switches.shade),
                None => facts.color,
            }),
            repo_dir: Some(
                place
                    .as_ref()
                    .map_or_else(|| dir.clone(), git::GitPlace::main_clone),
            ),
            branch: place
                .as_ref()
                .filter(|p| p.on_branch())
                .map(|p| p.branch.clone()),
            on_default: place
                .as_ref()
                .is_some_and(|p| p.on_branch() && git::is_default_branch(&p.common_dir, &p.branch)),
            worktree: place
                .filter(|p| p.worktree().is_some())
                .and_then(|_| git::top_level(&dir)),
            path: Some(dir),
            gone: false,
        };
        if self.tab_names.shown.get(&tab_id) != Some(&shown) {
            self.tab_names.shown.insert(tab_id, shown);
            if self
                .tabs
                .get(&tab_id)
                .is_some_and(|t| t.opts.contains_key(super::beside::OPTION))
            {
                self.on_task_soon(move |server| server.beside_check(tab_id));
            }
            if let Some(name) = self.session_of_tab(tab_id).map(|s| s.name.clone()) {
                self.dirty_session(&name);
            }
        }
        let auto_name = self.tab_facts_label(tab_id);
        if let Some(tab) = self.tabs.get_mut(&tab_id) {
            tab.auto_name = auto_name;
        }
    }

    /// Gives a tab with no name of its own the name it showed in a worktree
    /// that was removed under its pane, as the pane arrives in that
    /// repository's main clone (`dir`): a landed worktree's tab goes on
    /// saying what it did, rather than taking the main clone's name. The
    /// name is its `@tab_name`, so it is saved with the layout, and `/wt`,
    /// `wt` and a rename replace it as they do any other.
    fn keep_cleaned_name(&mut self, tab_id: TabId, dir: &Path) {
        let Some(shown) = self.tab_names.shown.get(&tab_id) else {
            return;
        };
        let worktree_removed = shown.gone && shown.worktree.as_ref().is_some_and(|w| !w.exists());
        if !worktree_removed || shown.label.is_empty() || self.tab_own_name(tab_id).is_some() {
            return;
        }
        let Some(place) = git::locate(dir) else {
            return;
        };
        let back_in_its_main_clone =
            place.worktree().is_none() && shown.repo_dir == Some(place.main_clone());
        if !back_in_its_main_clone {
            return;
        }
        let name = self.tab_facts_without_pr_title(tab_id);
        if let Some(tab) = self.tabs.get_mut(&tab_id) {
            tab.opts.insert("@tab_name".into(), name);
        }
    }

    /// The tab's own tint (`@tab_hue`, `@tab_step`), given as it is first
    /// named: the one farthest from every other open tab's, in every
    /// session, starting from its directory's (`home`), kept in its options
    /// so it is saved with the layout and never changes while it is open.
    fn own_tint(&mut self, tab_id: TabId, home: color::Tint) -> Option<color::Tint> {
        if let Some(tint) = color::Tint::of_opts(&self.tabs.get(&tab_id)?.opts) {
            return Some(tint);
        }
        let taken: Vec<color::Tint> = self
            .tabs
            .iter()
            .filter(|(id, _)| **id != tab_id)
            .filter_map(|(_, tab)| color::Tint::of_opts(&tab.opts))
            .collect();
        let tint = color::apart(&taken, home);
        let opts = &mut self.tabs.get_mut(&tab_id)?.opts;
        opts.insert(color::HUE.into(), tint.hue.to_string());
        opts.insert(color::STEP.into(), tint.step.name().into());
        Some(tint)
    }

    /// Names every tab again, and forgets what was kept for tabs and
    /// directories that are gone.
    pub fn name_all_tabs(&mut self) {
        let ids: Vec<TabId> = self.tabs.keys().copied().collect();
        for id in ids {
            self.name_tab(id);
        }
        let tabs = &self.tabs;
        self.tab_names.shown.retain(|id, _| tabs.contains_key(id));
        self.tab_names.names.prune(Instant::now());
    }

    /// A lookup's answer, from `Event::TabLookup`: the tabs are named again
    /// when it changed what they show.
    pub fn tab_lookup(&mut self, answer: Answer) {
        let now = Instant::now();
        if self.tab_names.names.answer(answer, now) {
            let saved = self.tab_names.names.saved(now, util::now_secs());
            let _ = util::write_atomic(names_file(), saved.to_string());
            self.name_all_tabs();
        }
    }

    // Runs a lookup off the server task, answering with `Event::TabLookup`.
    // A branch's PR isn't asked for while GitHub's lookups are slowed down
    // for its hour's limit (`sources::schedule::Budget`): the PR board's
    // answer still names it, and the lookup, unanswered, is asked again
    // once it counts as lost.
    fn start_lookup(&self, ask: Ask) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let left = crate::integrations::github::tightest_limit().map(|l| l.left());
        if matches!(ask, Ask::Pr { .. })
            && crate::sources::schedule::Budget::of(left)
                != crate::sources::schedule::Budget::Plenty
        {
            return;
        }
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let answer = ask.run().await;
            let _ = tx.send(Event::TabLookup(answer));
        });
    }

    /// The tab's own name: its `@tab_name` option (what M-p's "Rename the
    /// tab", `/shape` and `/wt` for a note's item set, and `/wt` for a ticket clears), first, as
    /// tab-label.sh read it, else the one given with `rename-tab`.
    pub fn tab_own_name(&self, tab: TabId) -> Option<&str> {
        let tab = self.tabs.get(&tab)?;
        tab.opts
            .get("@tab_name")
            .map(String::as_str)
            .filter(|n| !n.is_empty())
            .or_else(|| (!tab.name.is_empty()).then_some(tab.name.as_str()))
    }

    /// The tab's label (what tmux kept in `@tab_label`): its own name, else
    /// its ticket and title, else its directory's name, with `#<PR> ` in
    /// front when its branch has one. Stored as it is: nothing here reads a
    /// `#` as a format, so none is doubled.
    pub fn tab_label(&self, tab: TabId) -> String {
        let facts = self.tab_facts_label(tab);
        label::label(self.tab_own_name(tab), &facts, self.tab_pr(tab))
    }

    /// `tab_label` with its directory's name in place of a PR's title
    /// (`@tab_label_without_pr_title`), for what reads the label for a
    /// ticket's ID, pins it as the tab's name or hands it to Claude: a PR's
    /// title is anyone's words (`Fix UTF-8 paths`), never a ticket.
    pub fn tab_label_without_pr_title(&self, tab: TabId) -> String {
        let facts = self.tab_facts_without_pr_title(tab);
        label::label(self.tab_own_name(tab), &facts, self.tab_pr(tab))
    }

    /// `tab_facts_label` with its directory's name in place of a PR's title.
    fn tab_facts_without_pr_title(&self, tab: TabId) -> String {
        self.tab_names
            .shown
            .get(&tab)
            .map(|s| s.untitled.clone().unwrap_or_else(|| s.label.clone()))
            .or_else(|| self.tabs.get(&tab).map(|t| t.auto_name.clone()))
            .unwrap_or_default()
    }

    /// The tab's label with no name of its own and no PR number: its
    /// ticket and title, else its current PR's title, else its directory's
    /// name.
    fn tab_facts_label(&self, tab: TabId) -> String {
        let shown = self.tab_names.shown.get(&tab);
        let by_pr = shown.and_then(|s| s.untitled.as_deref()).map(|untitled| {
            match self.tab_pr_title(tab) {
                Some(title) => label::pr_label(untitled, &title),
                None => untitled.to_string(),
            }
        });
        by_pr
            .or_else(|| shown.map(|s| s.label.clone()))
            .or_else(|| self.tabs.get(&tab).map(|t| t.auto_name.clone()))
            .unwrap_or_default()
    }

    /// The color the tab wears: its project's, or gray with the
    /// `accent_colors` setting off; `None` before its directory is known.
    pub fn tab_color(&self, tab: TabId) -> Option<Rgb> {
        self.tab_names.shown.get(&tab)?.color
    }

    /// The directory the tab's name was worked out from (`@tab_path`).
    pub fn tab_path(&self, tab: TabId) -> Option<&Path> {
        self.tab_names.shown.get(&tab)?.path.as_deref()
    }

    /// The tab's current PR's number (`tabs::prs`): the one picked, else
    /// its branch's, else the newest its session opened.
    pub fn tab_pr(&self, tab: TabId) -> Option<u64> {
        self.tab_current_pr(tab).map(|p| p.number)
    }

    /// The title of the tab's current PR, once known: its branch's as the
    /// tab names last found it, another's as the PR lookups hold it.
    pub fn tab_pr_title(&self, tab: TabId) -> Option<String> {
        let current = self.tab_current_pr(tab)?;
        if self.tab_branch_pr(tab).is_some_and(|b| b.same(&current)) {
            return self.tab_names.shown.get(&tab)?.pr_title.clone();
        }
        self.tab_pr_facts(&current)
            .map(|f| f.title.clone())
            .filter(|t| !t.trim().is_empty())
    }

    /// The PR of the tab's branch, as the tab names last found it.
    fn tab_branch_pr(&self, tab: TabId) -> Option<TabPr> {
        let shown = self.tab_names.shown.get(&tab)?;
        Some(TabPr {
            number: shown.pr?,
            repo: shown.pr_repo.clone(),
        })
    }

    /// Every PR the tab follows, in the order opened (`tabs::prs::joined`):
    /// its branch's alone with Settings' `tab_prs` or `github` off.
    pub fn tab_prs(&self, tab: TabId) -> Vec<TabPr> {
        let branch = self.tab_branch_pr(tab);
        let opened = self
            .tabs
            .get(&tab)
            .and_then(|t| t.opts.get(prs::LIST))
            .filter(|_| crate::settings::is_on(prs::SETTING) && crate::settings::is_on("github"))
            .map(|v| prs::read(v))
            .unwrap_or_default();
        prs::joined(branch.as_ref(), &opened)
    }

    /// The tab's current PR among `tab_prs`.
    pub fn tab_current_pr(&self, tab: TabId) -> Option<TabPr> {
        let all = self.tab_prs(tab);
        let picked = self
            .tabs
            .get(&tab)
            .and_then(|t| t.opts.get(prs::CURRENT))
            .and_then(|v| TabPr::parse(v));
        let made = self
            .tabs
            .get(&tab)
            .and_then(|t| t.opts.get(prs::MADE))
            .map(|v| prs::read(v))
            .unwrap_or_default();
        let eligible = |pr: &TabPr| {
            made.iter().any(|m| m.same(pr))
                || self
                    .tab_pr_facts(pr)
                    .is_some_and(|f| f.state == crate::facts::PrState::Open)
        };
        prs::current(
            &all,
            self.tab_branch_pr(tab).as_ref(),
            picked.as_ref(),
            eligible,
        )
    }

    /// Whether `pr`, among the tab's PRs `all`, is a merged one its session
    /// made that the lookups keep on its list where the tab is now
    /// (`prs::merged_stays_current`), current as long as it stays there.
    fn landed_pr_stays(&self, tab: TabId, pr: &TabPr, all: &[TabPr]) -> bool {
        let Some(t) = self.tabs.get(&tab) else {
            return false;
        };
        let made = prs::read(t.opts.get(prs::MADE).map_or("", String::as_str));
        let Some(facts) = self
            .tab_pr_facts(pr)
            .filter(|f| f.state == crate::facts::PrState::Merged)
        else {
            return false;
        };
        if !made.iter().any(|m| m.same(pr)) {
            return false;
        }
        let shown = self.tab_names.shown.get(&tab);
        // Origin's, or in a fork's clone upstream's too, where its PRs open.
        let repos = self
            .tab_path(tab)
            .and_then(|p| self.sources.repos_kept(p))
            .filter(|r| !r.is_empty());
        let same_repo = match (repos, &pr.repo) {
            (Some(repos), Some(repo)) => {
                let repo = repo.to_string();
                repos.iter().any(|r| r.eq_ignore_ascii_case(&repo))
            }
            _ => true,
        };
        let another_open = all.iter().any(|p| {
            !p.same(pr)
                && made.iter().any(|m| m.same(p))
                && self
                    .tab_pr_facts(p)
                    .is_some_and(|f| f.state == crate::facts::PrState::Open)
        });
        prs::merged_stays_current(
            &facts.head,
            &prs::Place {
                branch: shown.and_then(|s| s.branch.as_deref()),
                on_default: shown.is_some_and(|s| s.on_default),
                same_repo,
                another_open,
            },
        )
    }

    /// The tab's current PR when it isn't its branch's and its repository is
    /// known, which M-v opens by its number; `None` when the branch's PR is
    /// current, as before.
    pub fn tab_current_pr_elsewhere(&self, tab: TabId) -> Option<TabPr> {
        let current = self.tab_current_pr(tab)?;
        let branch = self.tab_branch_pr(tab);
        (current.repo.is_some() && branch.is_none_or(|b| !b.same(&current))).then_some(current)
    }

    /// `tab_current_pr_elsewhere` when you picked it, which M-d opens where
    /// its branch is checked out; a PR current only by being newest leaves
    /// M-d on the tab's own work.
    pub fn tab_picked_pr_elsewhere(&self, tab: TabId) -> Option<TabPr> {
        let picked = self
            .tabs
            .get(&tab)
            .and_then(|t| t.opts.get(prs::CURRENT))
            .and_then(|v| TabPr::parse(v))?;
        self.tab_current_pr_elsewhere(tab)
            .filter(|c| c.same(&picked))
    }

    /// Where a PR's branch is checked out, for M-d on a tab's current PR
    /// from another branch: an open tab's directory, else a worktree mission
    /// control has listed, on the PR's head branch in a clone of its
    /// repository (origin's or upstream's); else why not, in words. Reads
    /// git's files, so only on a key, never on the drawing path.
    pub fn checkout_of_pr(&self, pr: &TabPr) -> Result<PathBuf, String> {
        let head = self
            .tab_pr_facts(pr)
            .map(|f| f.head.clone())
            .filter(|h| !h.is_empty())
            .ok_or_else(|| format!("#{}'s branch isn't known yet: M-v shows the PR", pr.number))?;
        let repo = pr
            .repo
            .as_ref()
            .ok_or_else(|| format!("#{}'s repository isn't known", pr.number))?;
        let same = |r: &Option<crate::integrations::github::Repo>| {
            r.as_ref().is_some_and(|r| {
                r.owner.eq_ignore_ascii_case(&repo.owner) && r.name.eq_ignore_ascii_case(&repo.name)
            })
        };
        let tabs = self
            .sessions
            .iter()
            .flat_map(|s| s.tabs.clone())
            .filter_map(|t| self.tab_path(t).map(Path::to_path_buf));
        let trees = self
            .mission
            .worktrees
            .values()
            .flatten()
            .map(|w| w.path.clone());
        tabs.chain(trees)
            .find(|dir| {
                git::locate(dir).is_some_and(|p| p.branch == head) && {
                    let remotes = crate::sources::repo::remotes_of(dir);
                    same(&remotes.origin) || same(&remotes.upstream)
                }
            })
            .ok_or_else(|| {
                format!(
                    "#{}'s branch, {head}, isn't checked out here: M-v shows the PR",
                    pr.number
                )
            })
    }

    /// With several PRs, the current one's place in the order opened (from
    /// 1), how many, and its number, as the tab's header says them.
    pub fn tab_pr_place(&self, tab: TabId) -> Option<(usize, usize, u64)> {
        let all = self.tab_prs(tab);
        if all.len() < 2 {
            return None;
        }
        let current = self.tab_current_pr(tab)?;
        let at = all.iter().position(|p| p.same(&current))?;
        Some((at + 1, all.len(), current.number))
    }

    /// Adds the PRs a tab's session opened to the ones it follows
    /// (`@tab_prs`), with Settings' `tab_prs` and `github` on.
    pub fn follow_tab_prs(&mut self, tab: TabId, opened: Vec<crate::agent::prs::Opened>) {
        if opened.is_empty()
            || !crate::settings::is_on(prs::SETTING)
            || !crate::settings::is_on("github")
        {
            return;
        }
        let Some(t) = self.tabs.get_mut(&tab) else {
            return;
        };
        let mut list = prs::read(t.opts.get(prs::LIST).map_or("", String::as_str));
        let mut made = prs::read(t.opts.get(prs::MADE).map_or("", String::as_str));
        let mut changed = false;
        for opened in opened {
            let pr = TabPr {
                number: opened.number,
                repo: Some(opened.repo),
            };
            if opened.made
                && let Some(more) = prs::with(&made, pr.clone())
            {
                made = more;
                changed = true;
            }
            if let Some(more) = prs::with(&list, pr) {
                list = more;
                changed = true;
            }
        }
        if changed {
            t.opts.insert(prs::LIST.into(), prs::write(&list));
            t.opts.insert(prs::MADE.into(), prs::write(&made));
            self.dirty_tab_everywhere(tab);
            self.beside_check(tab);
        }
    }

    /// "Show another of this tab's PRs" (board 129): a picker of the PRs the
    /// tab follows, in the order opened, the current one chosen as it
    /// opens, each with its repository, title and where it stands; the one
    /// picked becomes current.
    pub fn pick_from_tab_prs(&mut self, client: ClientId, tab: TabId) {
        let all = self.tab_prs(tab);
        if all.len() < 2 {
            let said = match all.first() {
                Some(pr) => format!("This tab follows one PR, #{}", pr.number),
                None => "This tab follows no PR".to_string(),
            };
            self.say(Some(client), &said);
            return;
        }
        let current = self.tab_current_pr(tab);
        let mut items = Vec::new();
        let mut start = 0;
        for (i, pr) in all.iter().enumerate() {
            let facts = self.tab_pr_facts(pr);
            let repo = pr.repo.as_ref().map(|r| r.name.clone()).unwrap_or_default();
            let title = facts.map(|f| f.title.as_str()).unwrap_or("");
            let is_current = current.as_ref().is_some_and(|c| c.same(pr));
            if is_current {
                start = i;
            }
            items.push(crate::ui::PickItem {
                text: format!("#{}  {repo}  {title}", pr.number),
                tags: facts.map(prs::standing_long).unwrap_or_default(),
                key: if is_current {
                    "current".into()
                } else {
                    String::new()
                },
                value: pr.key(),
                ..Default::default()
            });
        }
        let count = items.len();
        let picker = crate::ui::Picker::new("Show another of this tab's PRs", items)
            .named("tab-prs")
            .prompt(format!("{count} PRs> "))
            .keep_order()
            .start_on(start);
        self.pick(client, picker, move |server, client, picked| {
            let Some(picked) = picked else {
                return;
            };
            let chosen = server
                .tab_prs(tab)
                .into_iter()
                .find(|p| p.key() == picked.value);
            if let Some(pr) = chosen {
                server.pick_tab_pr(tab, &pr);
                server.reopen_on_current_pr(client, tab);
            }
        });
    }

    /// The PR view or the diff open in the tab in front of `client`, opened
    /// again on the tab's current PR once another is picked, so a pick from
    /// their own menus shows the PR picked.
    fn reopen_on_current_pr(&mut self, client: ClientId, tab: TabId) {
        let in_front = self
            .clients
            .get(&client)
            .and_then(|c| self.session(&c.session))
            .is_some_and(|s| s.current == tab);
        if !in_front {
            return;
        }
        if let Some(view) = self.marked_in(tab, crate::views::PR_VIEW) {
            self.kill_pane(view);
            self.view_command(client, crate::views::Command::PrView);
        }
        if let Some(diff) = self.diff_here(client) {
            self.kill_pane(diff);
            self.diff_command(client, crate::diff::Command::Diff);
        }
    }

    /// Makes `pr` the tab's current PR, for M-v, M-d and its label.
    pub fn pick_tab_pr(&mut self, tab: TabId, pr: &TabPr) {
        if let Some(t) = self.tabs.get_mut(&tab) {
            t.opts.insert(prs::CURRENT.into(), pr.key());
            self.dirty_tab_everywhere(tab);
        }
    }

    /// Takes the merged and closed PRs (`done`) a tab's session opened off
    /// its list once they have been seen so: a lookup's answer while the
    /// tab is in front of someone marks each, and the next answer takes the
    /// marked ones off. The branch's PR, never stored, shows as it always
    /// has, and a merged one the session made stays while it is the tab's
    /// current PR (`prs::merged_stays_current`), so a tab whose PR landed
    /// keeps it once the landing has moved it back to the main clone.
    pub fn drop_finished_tab_prs(&mut self, done: impl Fn(&TabPr) -> bool) {
        let ids: Vec<TabId> = self
            .tabs
            .iter()
            .filter(|(_, t)| t.opts.get(prs::LIST).is_some_and(|v| !v.is_empty()))
            .map(|(id, _)| *id)
            .collect();
        let mut seen = std::mem::take(&mut self.tab_names.finished_seen);
        seen.retain(|(id, _)| ids.contains(id));
        for id in ids {
            let in_front = self.tab_in_front(id);
            let all = self.tab_prs(id);
            let landed = self
                .tab_current_pr(id)
                .filter(|c| self.landed_pr_stays(id, c, &all));
            let t = self.tabs.get_mut(&id).expect("listed above");
            let list = prs::read(t.opts.get(prs::LIST).map_or("", String::as_str));
            let done = |p: &TabPr| done(p) && !landed.as_ref().is_some_and(|l| l.same(p));
            let mut left = Vec::new();
            for pr in list.iter().filter(|p| done(p)) {
                let key = (id, pr.key());
                if !seen.contains(&key) {
                    if in_front {
                        seen.insert(key);
                    }
                    left.push(pr.clone());
                }
            }
            let left: Vec<TabPr> = list
                .iter()
                .filter(|p| !done(p) || left.contains(p))
                .cloned()
                .collect();
            if left.len() != list.len() {
                seen.retain(|(tab, key)| *tab != id || left.iter().any(|p| p.key() == *key));
                let made: Vec<TabPr> = prs::read(t.opts.get(prs::MADE).map_or("", String::as_str))
                    .into_iter()
                    .filter(|m| left.iter().any(|p| p.same(m)))
                    .collect();
                t.opts.insert(prs::MADE.into(), prs::write(&made));
                // A pick that left the list goes with it, so the PR viewed
                // again later isn't current by an old pick.
                let picked = t.opts.get(prs::CURRENT).and_then(|v| TabPr::parse(v));
                if picked.is_some_and(|p| !left.iter().any(|l| l.same(&p))) {
                    t.opts.remove(prs::CURRENT);
                }
                t.opts.insert(prs::LIST.into(), prs::write(&left));
                self.dirty_tab_everywhere(id);
            }
        }
        self.tab_names.finished_seen = seen;
    }

    /// Redraws whatever shows a tab: its session's header, the sidebars and
    /// mission control.
    fn dirty_tab_everywhere(&mut self, tab: TabId) {
        if let Some(name) = self.session_of_tab(tab).map(|s| s.name.clone()) {
            self.dirty_session(&name);
        }
        self.redraw_everywhere();
    }

    /// The ticket the tab's branch is for, its GitHub issue or Linear
    /// ticket, as its directory's facts last said (`tabs::names`).
    pub fn tab_ticket(&self, tab: TabId) -> Option<&crate::views::ticket::Ticket> {
        self.tab_names.shown.get(&tab)?.ticket.as_ref()
    }

    /// The repository the tab's PR is in, when its lookup said: origin's,
    /// or in a fork's clone the project upstream names.
    pub fn tab_pr_repo(&self, tab: TabId) -> Option<Repo> {
        self.tab_current_pr(tab)?.repo
    }

    /// A tab's full title where its name is a note item cut short: its
    /// `@tab_title`, else, for a tab opened before there was one, the
    /// autopilot's `@autopilot_item`, either only while the tab's
    /// name is still that item cut short (src/tabs/title.rs), or a GitHub
    /// issue's reference it starts with (`<owner>/<repo>#<n>: <title>`), as
    /// an earlier build named an issue's tab.
    pub fn tab_full_title(&self, tab: TabId) -> Option<String> {
        let opts = &self.tabs.get(&tab)?.opts;
        let name = opts.get("@tab_name").map(String::as_str).unwrap_or("");
        let issue = |whole: &str| !name.is_empty() && whole.starts_with(&format!("{name}: "));
        ["@tab_title", "@autopilot_item"]
            .iter()
            .filter_map(|k| opts.get(*k))
            .find(|whole| title::cut_of(name, whole) || issue(whole))
            .cloned()
    }

    /// M-T's box for a tab (src/tabs/title.rs): its whole title, and its
    /// branch, directory, PR and ticket, with Copy the title and Close; with
    /// the tab_title setting off, it says so.
    pub fn show_tab_title(&mut self, client: ClientId, tab: TabId) {
        if !crate::settings::is_on("tab_title") {
            self.say(
                Some(client),
                "Showing a tab's full title is off in Settings (Show a tab's full title)",
            );
            return;
        }
        if self.session_of_tab(tab).is_none() {
            return;
        }
        let number = self.tab_number(tab);
        let label = self.tab_label(tab);
        let path = self.tab_path(tab).map(Path::to_path_buf);
        let place = path.as_deref().and_then(git::locate);
        let branch = place.as_ref().map(|p| p.branch.clone());
        // A note item's whole first line, where the tab bar has it cut
        // short, else the item its branch is for, read from git's config.
        let whole = self.tab_full_title(tab).or_else(|| {
            let place = place.as_ref().filter(|p| p.on_branch())?;
            let config = std::fs::read_to_string(place.common_dir.join("config")).ok()?;
            let item = title::wt_item(&config, &place.branch)?;
            let own = self.tab_own_name(tab).unwrap_or("");
            title::cut_of(own, &item).then_some(item)
        });
        let full = match (whole, self.tab_pr(tab)) {
            (Some(whole), Some(n)) => format!("#{n} {whole}"),
            (Some(whole), None) => whole,
            (None, _) => label,
        };
        let pr_title = self.tab_pr_title(tab);
        let ticket = self
            .tab_ticket(tab)
            .map(crate::views::ticket::Ticket::short);
        let facts = title::facts(
            branch.as_deref(),
            path.as_deref().map(paths::tilde).as_deref(),
            self.tab_pr(tab).map(|n| (n, pr_title.as_deref())),
            ticket.as_deref(),
        );
        let menu = Menu::new(
            format!("Tab {number}"),
            vec![
                MenuOption::new("Copy the title").key('y'),
                MenuOption::new("Close"),
            ],
        )
        .body(title::body(&full, &facts));
        self.about_tab(client, tab, move |s| {
            s.choose(client, menu, move |server, client, chose| {
                if chose == Some(0) {
                    server.to_clipboard(Some(client), &full);
                    server.say(Some(client), "Copied the tab's title");
                }
            })
        });
    }
}
