//! What the tab bar knows about each directory, and the lookups behind it:
//! tmux/path-facts.sh's answer per directory (its color, its label with no
//! name set, its PR), tmux/window-pr.sh's PR per repository and branch, and
//! tmux/ticket-title.sh's title per ticket, kept in memory by the server
//! rather than in files under `~/.cache`.
//!
//! A directory's answer is kept while its branch stays the one it was
//! worked out on: ten minutes once it has a PR, whose number never changes,
//! one minute before, about as often as a branch with no PR is asked about
//! again. Working it out reads git's files and nothing else; a PR or title
//! not known yet, or gone stale, is asked for in the background (an `Ask`,
//! which the server runs off its task and answers with an `Answer`), and
//! what was known goes out meanwhile, so a tab is never held up by the
//! network. When an answer changes what is known, every directory's answer
//! is dropped, as the scripts emptied path-facts.sh's cache before renaming
//! the tabs, so a PR number or title shows as soon as it is known.
//!
//! How long each lookup's answer holds is the scripts' own: a PR found, or a
//! title, for a day; no PR for a minute, since a branch with none is usually
//! one whose agent is about to open one; no such ticket for an hour, since
//! that is usually a branch that only looks like it names one. A lookup that
//! fails (no network, no token, a rate limit) keeps what was known and is
//! tried again in a minute, so a blip never blanks a title already shown.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::color::{self, Rgb};
use super::git::{self, GitPlace};
use super::label;
use crate::integrations::{github, linear};
use crate::{net, settings};

/// How long a directory's answer holds once it has a PR, and before.
pub const FACTS_FOUND: Duration = Duration::from_secs(600);
pub const FACTS_MISSING: Duration = Duration::from_secs(60);
/// How long a PR found, or the lack of one, holds (window-pr.sh).
const PR_FOUND: Duration = Duration::from_secs(24 * 3600);
const PR_MISSING: Duration = Duration::from_secs(60);
/// How long a ticket's title, or there being no such ticket, holds
/// (ticket-title.sh).
const TITLE_FOUND: Duration = Duration::from_secs(24 * 3600);
const TITLE_MISSING: Duration = Duration::from_secs(3600);
/// When a lookup that failed is tried again.
const RETRY: Duration = Duration::from_secs(60);
/// A lookup not answered in this long is taken as lost and asked again, as
/// the scripts gave up a lock nobody had touched in two minutes.
const LOST: Duration = Duration::from_secs(120);
/// How often the settings are read again.
const SETTINGS_EVERY: Duration = Duration::from_secs(2);
/// How long a directory no tab has shown is remembered.
const FORGET: Duration = Duration::from_secs(3600);

/// The settings the names follow, as last read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Switches {
    /// `github`: PR numbers.
    pub github: bool,
    /// `linear`: ticket titles.
    pub linear: bool,
    /// `tab_lookups`: either of them in tab names.
    pub lookups: bool,
    /// `accent_colors`: a color per project, else gray.
    pub colors: bool,
}

impl Default for Switches {
    fn default() -> Switches {
        Switches {
            github: true,
            linear: true,
            lookups: true,
            colors: true,
        }
    }
}

impl Switches {
    /// Read from the settings file now, so a change made in Settings or by
    /// hand reaches the tabs within `SETTINGS_EVERY`.
    pub fn read() -> Switches {
        let all = settings::read_settings();
        let on = |key: &str| all.iter().find(|o| o.key == key).is_none_or(|o| o.is_on());
        Switches {
            github: on("github"),
            linear: on("linear"),
            lookups: on("tab_lookups"),
            colors: on("accent_colors"),
        }
    }

    fn prs(self) -> bool {
        self.lookups && self.github
    }

    fn tickets(self) -> bool {
        self.lookups && self.linear
    }
}

/// What the tab bar shows for one directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Facts {
    /// Its accent color (the setting not applied).
    pub color: Rgb,
    /// Its label when the tab has no name of its own.
    pub label: String,
    /// Its branch's pull request.
    pub pr: Option<u64>,
}

struct Kept {
    facts: Facts,
    /// The branch it was worked out on (`None` outside a repository).
    branch: Option<String>,
    switches: Switches,
    at: Instant,
}

/// A lookup's answer, and when to ask again.
struct Known<T> {
    value: T,
    next: Instant,
    /// When it was last asked for, while that is unanswered.
    asked: Option<Instant>,
}

impl<T> Known<T> {
    /// Whether to ask now, marking it asked if so.
    fn due(&mut self, now: Instant) -> bool {
        let waiting = self.asked.is_some_and(|at| now.duration_since(at) < LOST);
        if waiting || now < self.next {
            return false;
        }
        self.asked = Some(now);
        true
    }
}

/// A lookup to run in the background.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ask {
    /// The PR for `branch` in the repository whose shared git directory is
    /// `repo`, asked from `dir`, a checkout of it.
    Pr {
        dir: PathBuf,
        repo: PathBuf,
        branch: String,
    },
    /// A ticket's title.
    Title { id: String },
}

/// What a lookup found: `Err` when it failed, which keeps what was known.
#[derive(Clone, Debug)]
pub enum Answer {
    Pr {
        repo: PathBuf,
        branch: String,
        found: Result<Option<u64>, String>,
    },
    Title {
        id: String,
        found: Result<Option<String>, String>,
    },
}

impl Ask {
    /// Runs the lookup, through GitHub's and Linear's APIs.
    pub async fn run(self) -> Answer {
        match self {
            Ask::Pr { dir, repo, branch } => {
                let found = match github::pr_for_branch(&dir, "tab name: PR").await {
                    Ok(pr) => Ok(pr.map(|p| p.number)),
                    // A service's failure keeps what was known; anything else
                    // (no origin, a remote not on GitHub) is a definite "no
                    // PR", as gh failing was to window-pr.sh.
                    Err(e) if net::kind_of(&e).is_some() => Err(e.to_string()),
                    Err(_) => Ok(None),
                };
                Answer::Pr {
                    repo,
                    branch,
                    found,
                }
            }
            Ask::Title { id } => {
                let found = linear::ticket(&id, "tab name: ticket title")
                    .await
                    .map(|t| t.map(|t| t.title))
                    .map_err(|e| e.to_string());
                Answer::Title { id, found }
            }
        }
    }
}

/// The tab bar's memory: each directory's answer and each lookup's.
#[derive(Default)]
pub struct Names {
    kept: HashMap<PathBuf, Kept>,
    prs: HashMap<(PathBuf, String), Known<Option<u64>>>,
    titles: HashMap<String, Known<Option<String>>>,
    switches: Switches,
    switches_read: Option<Instant>,
    /// Set by `set_switches`: the settings file isn't read.
    pinned: bool,
}

impl Names {
    /// The settings as last read, read again when `SETTINGS_EVERY` has passed.
    pub fn switches(&mut self, now: Instant) -> Switches {
        if !self.pinned
            && self
                .switches_read
                .is_none_or(|at| now.duration_since(at) >= SETTINGS_EVERY)
        {
            self.switches = Switches::read();
            self.switches_read = Some(now);
        }
        self.switches
    }

    /// The settings as last read, without reading them.
    pub fn last_switches(&self) -> Switches {
        self.switches
    }

    /// Sets the settings for good, rather than reading them: for tests.
    pub fn set_switches(&mut self, switches: Switches) {
        self.switches = switches;
        self.pinned = true;
    }

    /// What the tab bar shows for `dir`, and the lookups to start for it.
    pub fn facts(&mut self, dir: &Path, now: Instant) -> (Facts, Vec<Ask>) {
        let switches = self.switches(now);
        let place = git::locate(dir);
        let branch = place.as_ref().map(|p| p.branch.clone());
        if let Some(kept) = self.kept.get_mut(dir) {
            let holds = if kept.facts.pr.is_some() {
                FACTS_FOUND
            } else {
                FACTS_MISSING
            };
            if kept.branch == branch
                && kept.switches == switches
                && now.duration_since(kept.at) < holds
            {
                return (kept.facts.clone(), Vec::new());
            }
        }
        let mut asks = Vec::new();
        let facts = self.work_out(dir, place.as_ref(), switches, now, &mut asks);
        self.kept.insert(
            dir.to_path_buf(),
            Kept {
                facts: facts.clone(),
                branch,
                switches,
                at: now,
            },
        );
        (facts, asks)
    }

    fn work_out(
        &mut self,
        dir: &Path,
        place: Option<&GitPlace>,
        switches: Switches,
        now: Instant,
        asks: &mut Vec<Ask>,
    ) -> Facts {
        let name = color::dir_name(dir);
        let color = color::accent_color(&color::color_key(&name, place));
        let window = color::window_name(&name, place);

        // Matches any team key; a branch that only looks like a ticket is
        // harmless: Linear has no such ticket, and the name is used as usual.
        let ticket = place
            .filter(|_| switches.tickets())
            .and_then(|p| linear::ticket_in(&p.branch));
        let title = ticket.as_ref().and_then(|id| {
            let known = self.titles.entry(id.clone()).or_insert(Known {
                value: None,
                next: now,
                asked: None,
            });
            if known.due(now) {
                asks.push(Ask::Title { id: id.clone() });
            }
            known.value.clone()
        });
        let label = label::facts_label(&window, ticket.as_deref().zip(title.as_deref()));

        // A `wtpr` worktree carries its number in its directory's name, which
        // needs no lookup.
        let pr = if !switches.prs() {
            None
        } else if let Some(n) = github::pr_number_from_dir(dir) {
            Some(n)
        } else {
            place.filter(|p| p.on_branch()).and_then(|p| {
                // Keyed on the repository rather than the worktree, so a
                // ticket's worktree and a wtpr one on the same branch share
                // one answer.
                let key = (p.common_dir.clone(), p.branch.clone());
                let known = self.prs.entry(key).or_insert(Known {
                    value: None,
                    next: now,
                    asked: None,
                });
                if known.due(now) {
                    asks.push(Ask::Pr {
                        dir: dir.to_path_buf(),
                        repo: p.common_dir.clone(),
                        branch: p.branch.clone(),
                    });
                }
                known.value
            })
        };
        Facts { color, label, pr }
    }

    /// Takes a lookup's answer in. Gives whether it changed what is known,
    /// in which case every directory's answer has been dropped and the tabs
    /// should be named again.
    pub fn answer(&mut self, answer: Answer, now: Instant) -> bool {
        let changed = match answer {
            Answer::Pr {
                repo,
                branch,
                found,
            } => {
                let known = self.prs.entry((repo, branch)).or_insert(Known {
                    value: None,
                    next: now,
                    asked: None,
                });
                take(known, found, now, PR_FOUND, PR_MISSING)
            }
            Answer::Title { id, found } => {
                let known = self.titles.entry(id).or_insert(Known {
                    value: None,
                    next: now,
                    asked: None,
                });
                take(known, found, now, TITLE_FOUND, TITLE_MISSING)
            }
        };
        if changed {
            self.kept.clear();
        }
        changed
    }

    /// Forgets directories not looked at in an hour.
    pub fn prune(&mut self, now: Instant) {
        self.kept
            .retain(|_, kept| now.duration_since(kept.at) < FORGET);
    }
}

// An answer into what is known: a definite one replaces it and holds for
// `found` or `missing`; a failure keeps it and is tried again in `RETRY`.
fn take<T: PartialEq>(
    known: &mut Known<Option<T>>,
    found: Result<Option<T>, String>,
    now: Instant,
    found_for: Duration,
    missing_for: Duration,
) -> bool {
    known.asked = None;
    match found {
        Ok(value) => {
            known.next = now
                + if value.is_some() {
                    found_for
                } else {
                    missing_for
                };
            let changed = known.value != value;
            known.value = value;
            changed
        }
        Err(_) => {
            known.next = now + RETRY;
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::git::tests::{Scratch, git, repo};
    use super::*;

    fn names(switches: Switches) -> (Names, Instant) {
        let now = Instant::now();
        let mut names = Names::default();
        names.set_switches(switches);
        (names, now)
    }

    #[test]
    fn a_ticket_branch_asks_for_its_pr_and_title_and_shows_them_once_known() {
        let scratch = Scratch::new("names");
        let dir = scratch.0.join("app");
        if !repo(&dir) {
            eprintln!("skipped: no git");
            return;
        }
        assert!(git(&dir, &["checkout", "-q", "-b", "eng-42-fix-login"]));
        let (mut names, now) = names(Switches::default());

        let (facts, asks) = names.facts(&dir, now);
        assert_eq!(facts.label, "app:eng-42-fix-login");
        assert_eq!(facts.pr, None);
        assert_eq!(
            facts.color,
            color::accent_color("app | branch:eng-42-fix-login")
        );
        let repo_dir = dir.join(".git");
        assert_eq!(
            asks,
            vec![
                Ask::Title {
                    id: "ENG-42".into()
                },
                Ask::Pr {
                    dir: dir.clone(),
                    repo: repo_dir.clone(),
                    branch: "eng-42-fix-login".into()
                },
            ]
        );
        // Asked once: the answer is awaited, not asked for again.
        let later = now + FACTS_MISSING + Duration::from_secs(1);
        assert_eq!(names.facts(&dir, later).1, Vec::new());

        let title = Answer::Title {
            id: "ENG-42".into(),
            found: Ok(Some("Fix the login".into())),
        };
        assert!(names.answer(title, now));
        let pr = Answer::Pr {
            repo: repo_dir.clone(),
            branch: "eng-42-fix-login".into(),
            found: Ok(Some(12)),
        };
        assert!(names.answer(pr, now));
        let (facts, asks) = names.facts(&dir, now);
        assert_eq!(facts.label, "ENG-42: Fix the login");
        assert_eq!(facts.pr, Some(12));
        assert!(asks.is_empty());

        // A failure keeps what was known.
        let failed = Answer::Pr {
            repo: repo_dir,
            branch: "eng-42-fix-login".into(),
            found: Err("GitHub couldn't be reached".into()),
        };
        assert!(!names.answer(failed, now));
        assert_eq!(names.facts(&dir, now).0.pr, Some(12));
    }

    #[test]
    fn the_answer_holds_while_the_branch_does() {
        let scratch = Scratch::new("hold");
        let dir = scratch.0.join("app");
        if !repo(&dir) {
            eprintln!("skipped: no git");
            return;
        }
        let (mut names, now) = names(Switches::default());
        assert_eq!(names.facts(&dir, now).0.label, "app");
        assert!(git(&dir, &["checkout", "-q", "-b", "feature"]));
        // A new branch is worked out again at once.
        assert_eq!(names.facts(&dir, now).0.label, "app:feature");
    }

    #[test]
    fn with_lookups_off_nothing_is_asked() {
        let scratch = Scratch::new("off");
        let dir = scratch.0.join("app");
        if !repo(&dir) {
            eprintln!("skipped: no git");
            return;
        }
        assert!(git(&dir, &["checkout", "-q", "-b", "eng-42-fix"]));
        for switches in [
            Switches {
                github: false,
                linear: false,
                ..Switches::default()
            },
            Switches {
                lookups: false,
                ..Switches::default()
            },
        ] {
            let (mut names, now) = names(switches);
            let (facts, asks) = names.facts(&dir, now);
            assert!(asks.is_empty(), "{switches:?}");
            assert_eq!(facts.label, "app:eng-42-fix");
            assert_eq!(facts.pr, None);
        }
        // Only GitHub off: the title is still asked for, the PR not.
        let (mut names, now) = names(Switches {
            github: false,
            ..Switches::default()
        });
        assert_eq!(
            names.facts(&dir, now).1,
            vec![Ask::Title {
                id: "ENG-42".into()
            }]
        );
    }

    #[test]
    fn a_wtpr_directory_has_its_number_without_asking() {
        let scratch = Scratch::new("wtpr");
        let dir = scratch.0.join("pr-77-fix");
        if !repo(&dir) {
            eprintln!("skipped: no git");
            return;
        }
        assert!(git(&dir, &["checkout", "-q", "-b", "someone-fix"]));
        let (mut names, now) = names(Switches::default());
        let (facts, asks) = names.facts(&dir, now);
        assert_eq!(facts.pr, Some(77));
        assert!(asks.is_empty());
    }

    #[test]
    fn a_lookup_not_answered_is_asked_again_once_lost() {
        let now = Instant::now();
        let mut known = Known {
            value: None::<u64>,
            next: now,
            asked: None,
        };
        assert!(known.due(now));
        assert!(!known.due(now + Duration::from_secs(60)));
        assert!(known.due(now + LOST));
        // Answered with nothing: asked again a minute later.
        assert!(!take(&mut known, Ok(None), now, PR_FOUND, PR_MISSING));
        assert!(!known.due(now + Duration::from_secs(30)));
        assert!(known.due(now + PR_MISSING));
    }
}
