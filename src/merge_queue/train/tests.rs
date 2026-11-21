//! Trains run against real git repositories (an origin, a clone of it whose
//! git directory holds the queue, and a clone the PRs are made in), with
//! GitHub played by `Fake`: a PR's own checks always pass, a train fails
//! when its tree holds `fail.txt`, and a merge squashes the PR onto
//! origin's main as GitHub would, refusing a head that moved.

use std::collections::HashMap;
use std::fs;
use std::sync::Mutex;

use super::*;
use crate::merge_queue::{self, Queued};
use crate::tabs::git::tests::{Scratch, git};

struct Fake {
    origin: PathBuf,
    prs: Mutex<HashMap<u64, PrNow>>,
    merged: Mutex<Vec<u64>>,
    /// How many merges to refuse before taking one.
    refusals: Mutex<u32>,
    /// Refuse once, after this many more merges have gone through.
    fail_after: Mutex<Option<u32>>,
    /// Heads with no checks at all.
    no_checks: Mutex<Vec<String>>,
    /// The base branch's required checks, as GitHub says them.
    required: Mutex<Option<Vec<String>>>,
    /// PRs GitHub refuses to merge.
    unmergeable: Mutex<Vec<u64>>,
    /// PRs GitHub answers 404 for.
    not_found: Mutex<Vec<u64>>,
    /// A train fails its first attempt whatever it holds, as a flaky test
    /// does, and passes once its failed jobs run again.
    flaky: Mutex<bool>,
    /// The runs whose failed jobs were run again.
    reruns: Mutex<Vec<String>>,
    /// Refuse to run failed jobs again.
    rerun_refused: Mutex<bool>,
    /// How many looks after a rerun still show the first attempt's failure.
    stale: Mutex<u32>,
    /// How many times a failed run is still running its other jobs.
    still_running: Mutex<u32>,
}

impl Forge for Fake {
    async fn pr(&self, number: u64) -> Result<PrNow> {
        if self.not_found.lock().unwrap().contains(&number) {
            return Err(crate::net::ServiceError::new(
                crate::net::ErrorKind::NotFound,
                "GitHub",
                "GitHub has no such thing",
            )
            .into());
        }
        self.prs
            .lock()
            .unwrap()
            .get(&number)
            .cloned()
            .ok_or_else(|| anyhow!("no PR {number}"))
    }

    async fn required(&self, _branch: &str) -> Result<Option<Vec<String>>> {
        Ok(self.required.lock().unwrap().clone())
    }

    async fn checks(&self, sha: &str, _required: Option<&[String]>) -> Result<Seen> {
        if self.no_checks.lock().unwrap().iter().any(|h| h == sha) {
            return Ok(Seen::Nothing);
        }
        let a_head = self.prs.lock().unwrap().values().any(|p| p.head == sha);
        let fails = git(
            &self.origin,
            &["cat-file", "-e", &format!("{sha}:fail.txt")],
        );
        // Each commit's CI is a run of its own.
        let run = format!("77-{}", &sha[..7]);
        let rerun = self.reruns.lock().unwrap().contains(&run);
        let flaky = *self.flaky.lock().unwrap() && !rerun;
        let stale = {
            let mut stale = self.stale.lock().unwrap();
            let left = rerun && *stale > 0;
            *stale = stale.saturating_sub(u32::from(left));
            left
        };
        Ok(if (fails || flaky || stale) && !a_head {
            Seen::Failed {
                // The rerun's job is a new one.
                job: Some(format!("{run}-{}", u8::from(rerun && !stale))),
                run,
                names: vec!["Tests".into()],
            }
        } else {
            Seen::Passed
        })
    }

    async fn run_finished(&self, _run: &str) -> Result<bool> {
        let mut running = self.still_running.lock().unwrap();
        let finished = *running == 0;
        *running = running.saturating_sub(1);
        Ok(finished)
    }

    async fn rerun(&self, run: &str) -> Result<()> {
        if *self.rerun_refused.lock().unwrap() {
            return Err(anyhow!("GitHub answered 403"));
        }
        self.reruns.lock().unwrap().push(run.to_string());
        Ok(())
    }

    async fn merge(&self, number: u64, head: &str) -> Result<(), MergeError> {
        if self.unmergeable.lock().unwrap().contains(&number) {
            return Err(MergeError::Refused(
                "At least 1 approving review is required".into(),
            ));
        }
        self.merge_squash(number, head).map_err(MergeError::Failed)
    }

    fn run_url(&self, run: &str) -> String {
        format!("https://ci/{run}")
    }
}

impl Fake {
    fn merge_squash(&self, number: u64, head: &str) -> Result<()> {
        {
            let mut after = self.fail_after.lock().unwrap();
            match *after {
                Some(0) => {
                    *after = None;
                    return Err(anyhow!("502 Bad Gateway"));
                }
                Some(n) => *after = Some(n - 1),
                None => {}
            }
        }
        {
            let mut refusals = self.refusals.lock().unwrap();
            if *refusals > 0 {
                *refusals -= 1;
                return Err(anyhow!("502 Bad Gateway"));
            }
        }
        let mut prs = self.prs.lock().unwrap();
        let pr = prs.get_mut(&number).ok_or_else(|| anyhow!("no PR"))?;
        if pr.head != head {
            return Err(anyhow!("Head branch was modified"));
        }
        let main = crate::worktree::git_at(&self.origin, &["rev-parse", "main"])?;
        let tree =
            crate::worktree::git_at(&self.origin, &["merge-tree", "--write-tree", &main, head])?;
        let tree = tree.lines().next().unwrap_or("").to_string();
        let squashed = crate::worktree::git_at(
            &self.origin,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
                "commit-tree",
                &tree,
                "-p",
                &main,
                "-m",
                &format!("#{number}"),
            ],
        )?;
        crate::worktree::git_at(&self.origin, &["update-ref", "refs/heads/main", &squashed])?;
        pr.state = PrState::Merged;
        self.merged.lock().unwrap().push(number);
        Ok(())
    }
}

struct World {
    _scratch: Scratch,
    work: PathBuf,
    repo: Repo,
    fake: Fake,
}

impl World {
    fn new(name: &str) -> Option<World> {
        let scratch = Scratch::new(&format!("train-{name}"));
        let origin = scratch.0.join("origin.git");
        let work = scratch.0.join("work");
        let clone = scratch.0.join("clone");
        let made = git(
            &scratch.0,
            &["init", "-q", "--bare", origin.to_str().unwrap()],
        ) && git(&origin, &["symbolic-ref", "HEAD", "refs/heads/main"])
            && git(
                &scratch.0,
                &[
                    "clone",
                    "-q",
                    origin.to_str().unwrap(),
                    work.to_str().unwrap(),
                ],
            )
            && git(&work, &["checkout", "-q", "-b", "main"])
            && {
                fs::write(work.join("shared.txt"), "base\n").unwrap();
                git(&work, &["add", "."])
            }
            && git(&work, &["commit", "-q", "-m", "first"])
            && git(&work, &["push", "-q", "origin", "main"])
            && git(
                &scratch.0,
                &[
                    "clone",
                    "-q",
                    origin.to_str().unwrap(),
                    clone.to_str().unwrap(),
                ],
            );
        if !made {
            eprintln!("skipped: no git");
            return None;
        }
        Some(World {
            repo: Repo {
                common: clone.join(".git"),
                branch: "main".into(),
            },
            fake: Fake {
                origin,
                prs: Mutex::new(HashMap::new()),
                merged: Mutex::new(Vec::new()),
                refusals: Mutex::new(0),
                fail_after: Mutex::new(None),
                no_checks: Mutex::new(Vec::new()),
                not_found: Mutex::new(Vec::new()),
                unmergeable: Mutex::new(Vec::new()),
                required: Mutex::new(None),
                flaky: Mutex::new(false),
                reruns: Mutex::new(Vec::new()),
                rerun_refused: Mutex::new(false),
                stale: Mutex::new(0),
                still_running: Mutex::new(0),
            },
            work,
            _scratch: scratch,
        })
    }

    /// PR `number`, a commit on main writing `file` with `text`, queued.
    fn pr(&self, number: u64, file: &str, text: &str) {
        let branch = format!("pr{number}");
        assert!(git(&self.work, &["fetch", "-q", "origin"]));
        assert!(git(
            &self.work,
            &["checkout", "-q", "-B", &branch, "origin/main"]
        ));
        self.commit(number, file, text);
        self.queue_pr(number);
    }

    /// Another commit on PR `number`'s branch, pushed.
    fn commit(&self, number: u64, file: &str, text: &str) {
        assert!(git(&self.work, &["checkout", "-q", &format!("pr{number}")]));
        fs::write(self.work.join(file), text).unwrap();
        assert!(git(&self.work, &["add", "."]));
        assert!(git(&self.work, &["commit", "-q", "-m", file]));
        let spec = format!("HEAD:refs/pull/{number}/head");
        assert!(git(&self.work, &["push", "-q", "-f", "origin", &spec]));
        let head = crate::worktree::git_at(&self.work, &["rev-parse", "HEAD"]).unwrap();
        self.fake.prs.lock().unwrap().insert(
            number,
            PrNow {
                state: PrState::Open,
                head,
                base: "main".into(),
                from_here: true,
            },
        );
    }

    /// PR `number` queued (again) at its head as it is now.
    fn queue_pr(&self, number: u64) {
        let head = self.fake.prs.lock().unwrap()[&number].head.clone();
        merge_queue::change(&self.repo.common, |q| {
            merge_queue::queue_pr(
                q,
                Queued {
                    number,
                    head,
                    since: number as i64,
                    path: format!("/w/{number}"),
                    pane: Some(format!("{number}")),
                    ..Queued::default()
                },
            )
        })
        .unwrap()
        .unwrap();
    }

    fn step(&self) -> Vec<Told> {
        util::block_on(step(&self.repo, &self.fake, 1_000)).unwrap()
    }

    fn queue(&self) -> Queue {
        merge_queue::read(&self.repo.common)
    }

    fn in_train(&self) -> Vec<u64> {
        let queue = self.queue();
        queue
            .train
            .map(|t| t.prs.iter().map(|p| p.number).collect())
            .unwrap_or_default()
    }

    fn merged(&self) -> Vec<u64> {
        self.fake.merged.lock().unwrap().clone()
    }

    fn branches(&self) -> String {
        crate::worktree::git_at(&self.fake.origin, &["branch", "--list", "ci/*"]).unwrap()
    }
}

#[test]
fn a_train_of_two_green_prs_merges_both_in_order() {
    let Some(w) = World::new("green") else {
        return;
    };
    w.pr(1, "one.txt", "1\n");
    w.pr(2, "two.txt", "2\n");
    assert!(w.step().is_empty());
    assert_eq!(w.in_train(), [1, 2]);
    let train = w.queue().train.unwrap();
    assert!(w.branches().contains(&train.branch), "the train is pushed");
    assert!(w.step().is_empty());
    assert!(w.merged().is_empty(), "one green look isn't enough");
    assert!(w.queue().train.unwrap().passed);
    let told = w.step();
    assert_eq!(w.merged(), [1, 2]);
    assert_eq!(
        told.iter().map(|t| t.number).collect::<Vec<_>>(),
        [1, 2],
        "each tab is told"
    );
    assert_eq!(told[0].pane.as_deref(), Some("1"));
    let queue = w.queue();
    assert!(queue.train.is_none() && queue.prs.is_empty());
    assert_eq!(queue.status(2), "merged");
    assert_eq!(w.branches(), "", "the train's branch is deleted");
    let tree = |at: &str| {
        crate::worktree::git_at(&w.fake.origin, &["rev-parse", &format!("{at}^{{tree}}")])
    };
    assert_eq!(
        tree("main").unwrap(),
        tree(&train.commit).unwrap(),
        "main ends on the tree the train tested"
    );
}

#[test]
fn a_red_train_of_four_splits_and_drops_only_the_failing_pr() {
    let Some(w) = World::new("red") else {
        return;
    };
    w.pr(1, "one.txt", "1\n");
    w.pr(2, "two.txt", "2\n");
    w.pr(3, "fail.txt", "3\n");
    w.pr(4, "four.txt", "4\n");
    w.pr(5, "five.txt", "5\n");
    w.step();
    assert_eq!(w.in_train(), [1, 2, 3, 4], "four at most");
    let told = w.step();
    assert_eq!(w.in_train(), [1, 2, 3, 4], "run again before it is split");
    assert_eq!(told.len(), 4, "each tab is told of the rerun");
    w.step();
    assert_eq!(w.queue().splits, [vec![1, 2], vec![3, 4]]);
    let mut told = Vec::new();
    for _ in 0..40 {
        if !w.queue().busy() {
            break;
        }
        told.extend(w.step());
    }
    assert_eq!(w.merged(), [1, 2, 4, 5]);
    let queue = w.queue();
    assert!(!queue.busy());
    assert!(
        queue
            .status(3)
            .starts_with("dropped CI failed on its train ci/train-"),
        "{}",
        queue.status(3)
    );
    assert!(queue.status(3).contains("Tests (https://ci/77-"));
    let dropped: Vec<&Told> = told
        .iter()
        .filter(|t| t.number == 3 && t.words.starts_with("dropped"))
        .collect();
    assert_eq!(dropped.len(), 1);
    assert!(dropped[0].words.starts_with("dropped from the merge queue"));
    assert_eq!(w.branches(), "");
}

#[test]
fn a_pr_whose_head_moved_during_its_train_goes_back_in_the_queue() {
    let Some(w) = World::new("moved") else {
        return;
    };
    w.pr(1, "one.txt", "1\n");
    w.pr(2, "two.txt", "2\n");
    w.step();
    assert_eq!(w.in_train(), [1, 2]);
    w.commit(1, "one.txt", "1 again\n");
    w.step();
    let told = w.step();
    assert!(w.merged().is_empty(), "nothing after it merges either");
    assert_eq!(told.len(), 1);
    assert!(told[0].words.contains("head moved"));
    let queue = w.queue();
    assert!(queue.train.is_none());
    assert_eq!(queue.line(1), Some(1));
    assert_eq!(queue.line(2), Some(2));
    w.queue_pr(1);
    assert_eq!(
        w.queue().line(1),
        Some(1),
        "queued again, it keeps its place"
    );
    w.step();
    assert_eq!(w.in_train(), [1, 2], "a new train, with its new head");
    w.step();
    w.step();
    assert_eq!(w.merged(), [1, 2]);
}

#[test]
fn a_merge_that_fails_after_one_in_its_train_merged_goes_on_from_there() {
    let Some(w) = World::new("partway") else {
        return;
    };
    w.pr(1, "one.txt", "1\n");
    w.pr(2, "two.txt", "2\n");
    w.step();
    w.step();
    let train = w.queue().train.unwrap().branch;
    // The first merge goes through, the second fails once.
    w.fake.fail_after.lock().unwrap().replace(1);
    w.step();
    assert_eq!(w.merged(), [1]);
    assert_eq!(w.queue().train.unwrap().branch, train, "the train is kept");
    w.step();
    assert_eq!(w.merged(), [1, 2], "the next step merges the rest");
}

#[test]
fn a_pr_github_has_none_of_is_dropped_and_one_it_fails_on_passed_over() {
    let Some(w) = World::new("unknown") else {
        return;
    };
    w.pr(1, "one.txt", "1\n");
    w.pr(2, "two.txt", "2\n");
    w.pr(3, "three.txt", "3\n");
    w.fake.prs.lock().unwrap().remove(&1);
    w.fake.not_found.lock().unwrap().push(2);
    w.step();
    assert_eq!(w.in_train(), [3], "neither stops the queue");
    assert_eq!(
        w.queue().status(1),
        "queued 2 of 2",
        "kept for a later look"
    );
    assert_eq!(
        w.queue().status(2),
        "dropped GitHub has no such pull request"
    );
}

#[test]
fn a_pr_github_won_t_merge_is_dropped_and_the_rest_go_on() {
    let Some(w) = World::new("unmergeable") else {
        return;
    };
    w.pr(1, "one.txt", "1\n");
    w.pr(2, "two.txt", "2\n");
    w.fake.unmergeable.lock().unwrap().push(1);
    w.step();
    w.step();
    w.step();
    let queue = w.queue();
    assert!(queue.held.is_none(), "the queue isn't held for one PR");
    assert_eq!(
        queue.status(1),
        "dropped GitHub won't merge it: At least 1 approving review is required"
    );
    for _ in 0..3 {
        w.step();
    }
    assert_eq!(w.merged(), [2]);
}

#[test]
fn a_pr_with_no_checks_goes_in_a_train_once_none_came() {
    let Some(w) = World::new("nochecks") else {
        return;
    };
    w.pr(1, "one.txt", "1\n");
    w.fake
        .no_checks
        .lock()
        .unwrap()
        .push(w.fake.prs.lock().unwrap()[&1].head.clone());
    let early = util::block_on(step(&w.repo, &w.fake, 1 + NO_CHECKS / 2)).unwrap();
    assert!(
        early.is_empty() && w.queue().train.is_none(),
        "it waits for checks"
    );
    util::block_on(step(&w.repo, &w.fake, 1 + NO_CHECKS * 2)).unwrap();
    assert_eq!(w.in_train(), [1]);
}

#[test]
fn a_pr_whose_required_checks_never_came_is_never_ready() {
    let Some(w) = World::new("required") else {
        return;
    };
    *w.fake.required.lock().unwrap() = Some(vec!["Tests".into()]);
    w.pr(1, "one.txt", "1\n");
    w.fake
        .no_checks
        .lock()
        .unwrap()
        .push(w.fake.prs.lock().unwrap()[&1].head.clone());
    util::block_on(step(&w.repo, &w.fake, 1 + NO_CHECKS * 2)).unwrap();
    assert!(
        w.queue().train.is_none(),
        "it waits for the checks its base requires"
    );
}

#[test]
fn a_failed_merge_is_tried_again_and_holds_the_queue_only_after_three() {
    let Some(w) = World::new("retry") else {
        return;
    };
    w.pr(1, "one.txt", "1\n");
    w.step();
    w.step();
    *w.fake.refusals.lock().unwrap() = 1;
    w.step();
    assert!(w.merged().is_empty() && w.queue().held.is_none());
    assert_eq!(w.queue().train.unwrap().merge_failures, 1);
    w.step();
    assert_eq!(w.merged(), [1], "the next step merges it");
    assert_eq!(*w.fake.refusals.lock().unwrap(), 0);

    w.pr(2, "two.txt", "2\n");
    w.step();
    w.step();
    *w.fake.refusals.lock().unwrap() = 3;
    for _ in 0..3 {
        w.step();
    }
    let queue = w.queue();
    assert!(queue.train.is_none());
    assert!(
        queue
            .status(2)
            .starts_with("held GitHub refused to merge #2"),
        "{}",
        queue.status(2)
    );
}

#[test]
fn only_the_head_queued_and_no_fork_s_branch_goes_in_a_train() {
    let Some(w) = World::new("heads") else {
        return;
    };
    w.pr(1, "one.txt", "1\n");
    w.pr(2, "two.txt", "2\n");
    w.pr(3, "three.txt", "3\n");
    w.commit(1, "one.txt", "pushed after it was queued\n");
    w.fake.prs.lock().unwrap().get_mut(&3).unwrap().from_here = false;
    w.pr(4, "four.txt", "4\n");
    w.pr(5, "five.txt", "5\n");
    w.pr(6, "six.txt", "6\n");
    let told = w.step();
    assert_eq!(
        w.in_train(),
        [2, 4, 5, 6],
        "the moved head waits to be queued again, taking no place"
    );
    assert_eq!(w.queue().line(1), Some(2));
    assert_eq!(told.len(), 1);
    assert!(
        w.queue()
            .status(3)
            .starts_with("dropped its head is a fork's branch"),
        "{}",
        w.queue().status(3)
    );
}

#[test]
fn a_red_train_of_one_queued_again_at_a_new_head_keeps_it_queued() {
    let Some(w) = World::new("requeued") else {
        return;
    };
    w.pr(1, "fail.txt", "1\n");
    w.step();
    assert_eq!(w.in_train(), [1]);
    w.commit(1, "fail.txt", "fixed\n");
    assert!(git(&w.work, &["rm", "-q", "fail.txt"]));
    assert!(git(&w.work, &["commit", "-q", "-m", "fixed"]));
    assert!(git(
        &w.work,
        &["push", "-q", "-f", "origin", "HEAD:refs/pull/1/head"]
    ));
    let head = crate::worktree::git_at(&w.work, &["rev-parse", "HEAD"]).unwrap();
    w.fake.prs.lock().unwrap().get_mut(&1).unwrap().head = head;
    w.queue_pr(1);
    w.step();
    let queue = w.queue();
    assert!(queue.train.is_none());
    assert_eq!(
        queue.status(1),
        "queued 1 of 1",
        "not dropped for the old head"
    );
    w.step();
    w.step();
    w.step();
    assert_eq!(w.merged(), [1], "its new head merges");
}

#[test]
fn a_pr_that_conflicts_with_one_ahead_is_dropped_from_the_train() {
    let Some(w) = World::new("conflict") else {
        return;
    };
    w.pr(1, "shared.txt", "one\n");
    w.pr(2, "shared.txt", "two\n");
    let told = w.step();
    assert_eq!(w.in_train(), [1]);
    assert_eq!(told.len(), 1);
    assert_eq!(
        w.queue().status(2),
        "dropped it doesn't merge cleanly onto main with #1 ahead of it"
    );
}

#[test]
fn a_pr_taken_out_ends_its_train() {
    let Some(w) = World::new("out") else {
        return;
    };
    w.pr(1, "one.txt", "1\n");
    w.pr(2, "two.txt", "2\n");
    w.step();
    merge_queue::change(&w.repo.common, |q| merge_queue::dequeue_pr(q, 2)).unwrap();
    w.step();
    assert!(w.queue().train.is_none());
    assert_eq!(w.branches(), "");
    w.step();
    assert_eq!(w.in_train(), [1]);
}

#[test]
fn a_405_or_422_is_a_refusal_but_a_base_just_modified_is_not() {
    let e = |words: &str| merge_error(anyhow!("{words}"));
    assert!(matches!(
        e("GitHub answered 405: At least 1 approving review is required"),
        MergeError::Refused(_)
    ));
    assert!(matches!(
        e("GitHub answered 422: draft"),
        MergeError::Refused(_)
    ));
    assert!(matches!(
        e("GitHub answered 405: Base branch was modified. Review and try the merge again."),
        MergeError::Failed(_)
    ));
    assert!(matches!(e("GitHub answered 502"), MergeError::Failed(_)));
    assert!(matches!(
        e("GitHub answered 405: Pull Request is not mergeable"),
        MergeError::Failed(_)
    ));
    assert!(matches!(
        e("GitHub answered 405: Head branch is not up to date with the base branch"),
        MergeError::Strict(_)
    ));
    assert!(matches!(
        e("GitHub answered 405: Head branch is out of date. Review and try the merge again."),
        MergeError::Strict(_)
    ));
}

#[test]
fn a_train_that_fails_once_and_passes_on_its_rerun_merges() {
    let Some(w) = World::new("flaky") else {
        return;
    };
    *w.fake.flaky.lock().unwrap() = true;
    *w.fake.stale.lock().unwrap() = 1;
    *w.fake.still_running.lock().unwrap() = 1;
    w.pr(1, "one.txt", "1\n");
    w.pr(2, "two.txt", "2\n");
    w.step();
    assert!(w.step().is_empty());
    assert!(
        w.fake.reruns.lock().unwrap().is_empty(),
        "no rerun while the run's other jobs still run"
    );
    assert_eq!(w.in_train(), [1, 2]);
    let told = w.step();
    let train = w.queue().train.unwrap();
    assert_eq!(w.fake.reruns.lock().unwrap().len(), 1);
    assert_eq!(train.rerun.as_ref().unwrap().names, ["Tests"]);
    assert_eq!(
        told[0].words,
        format!(
            "{} failed Tests (https://ci/77-{}): running its failed jobs again",
            train.branch,
            &train.commit[..7]
        )
    );
    w.step();
    assert_eq!(
        w.queue().train.unwrap().rerun,
        train.rerun,
        "the first attempt's failure, still showing, is no second failure"
    );
    for _ in 0..4 {
        w.step();
    }
    assert_eq!(w.merged(), [1, 2]);
    let queue = w.queue();
    assert!(queue.splits.is_empty() && queue.dropped.is_empty());
}

#[test]
fn a_train_of_one_red_on_both_attempts_drops_its_pr_and_is_run_again_once() {
    let Some(w) = World::new("twice") else {
        return;
    };
    w.pr(1, "fail.txt", "1\n");
    w.step();
    w.step();
    assert!(w.queue().train.unwrap().rerun.is_some());
    // Another job's failure while the rerun's run hasn't ended waits.
    *w.fake.still_running.lock().unwrap() = 1;
    w.step();
    assert!(
        w.queue().train.is_some(),
        "not dropped while the rerun runs"
    );
    w.step();
    assert!(w.queue().train.is_none());
    assert!(w.queue().status(1).starts_with("dropped CI failed"));
    assert_eq!(w.fake.reruns.lock().unwrap().len(), 1, "run again once");
}

#[test]
fn a_train_of_two_red_on_both_attempts_splits() {
    let Some(w) = World::new("twice-two") else {
        return;
    };
    w.pr(1, "one.txt", "1\n");
    w.pr(2, "fail.txt", "2\n");
    w.step();
    w.step();
    assert!(w.queue().splits.is_empty());
    w.step();
    assert_eq!(w.queue().splits, [vec![1], vec![2]]);
    assert_eq!(w.fake.reruns.lock().unwrap().len(), 1);
}

#[test]
fn a_rerun_github_refuses_drops_as_before() {
    let Some(w) = World::new("refused-rerun") else {
        return;
    };
    *w.fake.rerun_refused.lock().unwrap() = true;
    w.pr(1, "fail.txt", "1\n");
    w.step();
    w.step();
    assert!(w.queue().train.is_none());
    assert!(w.queue().status(1).starts_with("dropped CI failed"));
}
