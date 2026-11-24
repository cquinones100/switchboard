//! The merge queue on the server: on the tick, each repository whose queue
//! (as the landing read last found it, src/sources/landing.rs) holds a PR or
//! a train is stepped (`train::step`) at most every minute (`poll`), one step at a
//! time for each, off the server's task, since a step runs git and asks
//! GitHub. What a step tells a PR's tab shows as its pane's phase, which the
//! sidebar and the Landing view show and `/queue-and-clean`'s watch reads
//! through `land-lock pr-status`. A PR is queued only in a repository whose
//! way is Switchboard's merge queue (repos.json's `lands`), and stays
//! queued when that way changes, finishing the old way. With Settings'
//! `github` off, or in a `switchboard try`'s server, which knows none of
//! the real workspace's tabs, nothing is stepped and what is queued waits;
//! with the setting off, `land-lock pr-status` says the queue is held for
//! it (`merge_queue::stopped`), so a watch ends saying why.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::train::{self, Told};
use crate::mux::PaneId;
use crate::mux::server::{Event, Server};

/// How often a repository's queue is stepped, `SWITCHBOARD_MERGE_QUEUE_POLL`
/// seconds for the tests.
fn poll() -> Duration {
    std::env::var("SWITCHBOARD_MERGE_QUEUE_POLL")
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s >= 0.0)
        .map(Duration::from_secs_f64)
        .unwrap_or(Duration::from_secs(60))
}

/// Whether this server runs the queues.
fn runs() -> bool {
    let in_a_try = std::env::var("SWITCHBOARD_CHECKOUT").is_ok_and(|c| !c.is_empty());
    super::stopped().is_none() && !in_a_try
}

/// The steps under way and when each repository was last stepped.
#[derive(Debug, Default)]
pub struct MergeQueue {
    running: HashSet<PathBuf>,
    stepped: HashMap<PathBuf, Instant>,
}

/// A step's answer: what it tells the tabs, or why it failed (kept in the
/// queue's file for the Landing view; the next step tries again).
pub enum MergeQueueEvent {
    Stepped {
        common: PathBuf,
        told: Result<Vec<Told>, String>,
        /// The step recorded a PR merged, or failed, maybe after a merge, so
        /// main may have moved.
        merged: bool,
    },
}

impl Server {
    /// The merge queue's part of the tick.
    pub fn merge_queue_tick(&mut self) {
        if !runs() || tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let due: Vec<(PathBuf, String, Option<String>)> = self
            .sources
            .landing
            .repos
            .iter()
            .filter(|r| r.target.is_none() && r.prs.busy())
            .filter(|r| !self.merge_queue.running.contains(&r.common))
            .filter(|r| {
                self.merge_queue
                    .stepped
                    .get(&r.common)
                    .is_none_or(|at| at.elapsed() >= poll())
            })
            .filter_map(|r| Some((r.common.clone(), r.repo.clone()?, r.branch.clone())))
            .collect();
        for (common, repo, branch) in due {
            self.merge_queue.running.insert(common.clone());
            self.merge_queue
                .stepped
                .insert(common.clone(), Instant::now());
            let tx = self.tx.clone();
            let handle = tokio::runtime::Handle::current();
            tokio::task::spawn_blocking(move || {
                let last_merge =
                    |common: &PathBuf| super::read(common).merged.iter().map(|m| m.at).max();
                let before = last_merge(&common);
                // A clone with no origin/HEAD (made with git init and a
                // remote added) asks GitHub for its default branch.
                let branch = match branch {
                    Some(branch) => Ok(branch),
                    None => match crate::integrations::github::parse_name_with_owner(&repo) {
                        Some(r) => handle
                            .block_on(crate::integrations::github::default_branch(
                                &r,
                                train::SOURCE,
                            ))
                            .map_err(|e| format!("couldn't find the default branch: {e}")),
                        None => Err(format!("{repo} isn't a GitHub repository")),
                    },
                };
                let told = match branch {
                    Ok(branch) => {
                        let at = train::Repo {
                            common: common.clone(),
                            branch,
                        };
                        let forge = train::GitHub { repo };
                        handle
                            .block_on(train::step(&at, &forge, crate::util::now_ms()))
                            .map_err(|e| e.to_string())
                    }
                    Err(e) => Err(e),
                };
                // A step that went through keeps what it recorded itself.
                if let Err(e) = &told {
                    let _ = super::change(&common, |q| q.failed = Some(e.clone()));
                }
                // A step that failed may have merged on GitHub before it
                // failed (its fetch after the merge), recording nothing.
                let merged = told.is_err() || last_merge(&common) != before;
                let _ = tx.send(Event::MergeQueue(MergeQueueEvent::Stepped {
                    common,
                    told,
                    merged,
                }));
            });
        }
    }

    pub fn merge_queue_event(&mut self, event: MergeQueueEvent) {
        let MergeQueueEvent::Stepped {
            common,
            told,
            merged,
        } = event;
        self.merge_queue.running.remove(&common);
        // Main moved on GitHub: counted at once, not on the hour.
        if merged {
            self.behind_check();
        }
        // A failed step is in the queue's file already (`failed`).
        if let Ok(told) = told {
            for t in told {
                let Some(pane) = t.pane.as_deref().and_then(|p| p.parse::<PaneId>().ok()) else {
                    continue;
                };
                let now = crate::util::now_ms();
                let _ = self
                    .agents
                    .set_option(pane, "@agent_phase", Some(&t.words), now);
                let _ = self
                    .agents
                    .set_option(pane, "@agent_loop", Some("queue"), now);
            }
        }
        self.redraw_everywhere();
    }
}
