//! The default branch's CI, watched: for each repository whose landings
//! test in CI (repos.json's `landing` `direct` with `testIn` `ci`) and that
//! autopilot works on or has a tab in its landing queue, its default
//! branch's tip is looked at every two minutes (src/sources/schedule.rs),
//! with the look `wait-ci --now` makes (src/cmd/wait_ci.rs). A tip that
//! failed becomes an item of mission control's CI view, "<branch> failing
//! at <short hash>: <job>", with the line of its log that says why, its
//! notification (the `ci` kind, said once per failing commit), and the
//! Autopilot line's "<branch> failing at <short hash>". A later tip still
//! running, or with no CI, keeps the failure shown (`merged`), since it may
//! fail too; one that passes clears it.
//!
//! It is new with the Rust version. A landing whose tests run in CI cuts
//! from the default branch, so a failing one makes every landing's own run
//! fail too, and before this nothing read the default branch's CI: a push
//! that broke it went unheard of until the next landing tripped on it.
//! The `main_ci_watch` setting turns it off.

use serde_json::{Map, Value, json};

use super::ciboard::{Step, failing_step, read_log};
use crate::cmd::wait_ci::{Seen, look};
use crate::items::{Item, State, Target};

const SOURCE: &str = "sidebar: default branch CI";

/// A repository whose default branch is watched, by its `owner/name`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Watched {
    pub repo: String,
    pub branch: String,
}

/// Whether a repository's landings test in CI: its entry in repos.json
/// lands `direct` (set there, else as last worked out) with `testIn` `ci`.
pub fn tests_in_ci(data: &Map<String, Value>, repo: &str, worked_out: Option<&str>) -> bool {
    let entry = crate::repos::landing_entry(data, repo);
    let direct = entry.get("landing").and_then(Value::as_str).or(worked_out) == Some("direct");
    direct && crate::repos::steps_of(&entry).test_in == "ci"
}

/// The repositories to watch: of those `busy` names (autopilot works on
/// it, or a tab waits in its landing queue), with the default branch each
/// is on, those whose landings test in CI, each once.
pub fn watched(
    busy: &[(String, String)],
    data: &Map<String, Value>,
    worked_out: &dyn Fn(&str) -> Option<String>,
) -> Vec<Watched> {
    let mut out: Vec<Watched> = Vec::new();
    for (repo, branch) in busy {
        if out.iter().any(|w| w.repo.eq_ignore_ascii_case(repo))
            || !tests_in_ci(data, repo, worked_out(repo).as_deref())
        {
            continue;
        }
        out.push(Watched {
            repo: repo.clone(),
            branch: branch.clone(),
        });
    }
    out
}

/// A failed tip's item in the CI view; none for one passing, running or
/// with no CI. Its `ci` data is what the CI view's menu reads: no pull
/// request, the failing check's run, and its job's log once read, with
/// its failing step, as a pull request's failed check has it.
pub fn item_of(w: &Watched, sha: &str, seen: &Seen, step: Option<&Step>) -> Option<Item> {
    let why = step.map(|s| s.why.as_str());
    let Seen::Failed { run, names, job } = seen else {
        return None;
    };
    let short: String = sha.chars().take(7).collect();
    let repo = &w.repo;
    let check_url = if run == "-" {
        format!("https://github.com/{repo}/commit/{sha}")
    } else {
        format!("https://github.com/{repo}/actions/runs/{run}")
    };
    let first = names.first().cloned().unwrap_or_default();
    let mut detail = format!("{repo} ✗ {}", names.join(", "));
    if let Some(why) = why.filter(|w| !w.is_empty()) {
        detail.push_str(" · ");
        detail.push_str(why);
    }
    Some(
        Item::new(
            format!("main-ci:{repo}:{sha}"),
            format!("{} failing at {short}: {first}", w.branch),
        )
        .detail(detail)
        .state(State::Attention)
        .target(Target::Url(check_url.clone()))
        .data(json!({
            "ci": {
                "number": 0, "repo": repo, "url": "",
                "branch": w.branch,
                "check": { "name": first, "url": check_url, "job": job },
                "checks": [{
                    "name": first, "url": check_url, "job": job,
                    "run": (run != "-").then_some(run),
                    "why": why, "step": step.map(Step::to_json),
                }],
                "job": job,
                "logged": why.is_some(),
                "jobName": first,
            },
            "mainCi": { "repo": repo, "branch": w.branch, "sha": sha, "short": short },
            "search": repo,
        })),
    )
}

/// What one look found of a watched default branch: its failing tip's item,
/// and whether the tip has settled (passed or failed) rather than still
/// running or having no CI.
#[derive(Clone, Debug, PartialEq)]
pub struct Looked {
    pub repo: String,
    pub item: Option<Item>,
    pub settled: bool,
}

/// The failing default branches after a look: each one looked at as it now
/// is, but one whose tip hasn't settled keeps what it showed before; one no
/// longer watched goes.
pub fn merged(before: &[Item], looked: Vec<Looked>) -> Vec<Item> {
    let repo_of = |i: &Item| {
        i.data
            .pointer("/mainCi/repo")
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let mut out = Vec::new();
    for l in looked {
        if l.settled {
            out.extend(l.item);
        } else {
            out.extend(
                before
                    .iter()
                    .filter(|i| repo_of(i).is_some_and(|r| r.eq_ignore_ascii_case(&l.repo)))
                    .cloned(),
            );
        }
    }
    out
}

/// Looks at each watched default branch's tip, and why each failing one
/// failed. One that can't be asked about fails the whole look, so what was
/// known stays rather than a failing branch seeming to pass.
pub async fn fetch(watched: Vec<Watched>) -> anyhow::Result<Vec<Looked>> {
    let mut found = Vec::new();
    for w in &watched {
        let (sha, seen) = look(&w.repo, &w.branch, SOURCE).await?;
        let step = match &seen {
            Seen::Failed { job: Some(job), .. } => {
                read_log(&w.repo, job).await.map(|log| failing_step(&log))
            }
            _ => None,
        };
        found.push(Looked {
            repo: w.repo.clone(),
            settled: matches!(seen, Seen::Passed | Seen::Failed { .. }),
            item: item_of(w, &sha, &seen, step.as_ref()),
        });
    }
    Ok(found)
}

/// What the Autopilot line says while a watched default branch fails:
/// "main failing at abc1234", one for each.
pub fn failing_line(items: &[Item]) -> String {
    items
        .iter()
        .filter_map(|i| {
            let m = i.data.get("mainCi")?;
            Some(format!(
                "{} failing at {}",
                m.get("branch")?.as_str()?,
                m.get("short")?.as_str()?
            ))
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn main_of(repo: &str) -> Watched {
        Watched {
            repo: repo.into(),
            branch: "main".into(),
        }
    }

    #[test]
    fn a_failed_tip_is_a_ci_item_with_its_job_and_why_and_a_passing_or_running_one_none() {
        let w = main_of("acme/widgets");
        let failed = Seen::Failed {
            run: "77".into(),
            names: vec!["Clippy and tests (macOS)".into()],
            job: Some("5".into()),
        };
        let step = Step {
            why: "FAIL views".into(),
            ..Step::default()
        };
        let item = item_of(&w, "ebde129e0000", &failed, Some(&step)).unwrap();
        assert_eq!(
            item.title,
            "main failing at ebde129: Clippy and tests (macOS)"
        );
        assert_eq!(
            item.detail,
            "acme/widgets ✗ Clippy and tests (macOS) · FAIL views"
        );
        assert_eq!(item.data["ci"]["job"], "5");
        assert_eq!(item.data["ci"]["logged"], true);
        assert_eq!(
            item.data["ci"]["check"]["url"],
            "https://github.com/acme/widgets/actions/runs/77"
        );
        assert_eq!(item.data["ci"]["url"], "", "no pull request to offer");
        assert_eq!(
            item.data["ci"]["checks"][0]["why"], "FAIL views",
            "the CI view's line that says why, as a PR's check has it"
        );
        assert_eq!(item.data["ci"]["checks"][0]["run"], "77");
        assert_eq!(
            failing_line(std::slice::from_ref(&item)),
            "main failing at ebde129"
        );
        for seen in [Seen::Passed, Seen::Running, Seen::Nothing] {
            assert!(item_of(&w, "abc", &seen, None).is_none());
        }
        let looked = |settled: bool, item: Option<Item>| Looked {
            repo: "acme/widgets".into(),
            item,
            settled,
        };
        let before = vec![item.clone()];
        assert_eq!(
            merged(&before, vec![looked(false, None)]),
            before,
            "a later tip still running keeps the failure"
        );
        assert!(
            merged(&before, vec![looked(true, None)]).is_empty(),
            "a passing one clears it"
        );
        assert!(merged(&before, Vec::new()).is_empty(), "no longer watched");
    }

    #[test]
    fn only_repositories_whose_landings_test_in_ci_are_watched_each_once() {
        let data = json!({
            "acme/ci": { "landing": "direct", "testIn": "ci" },
            "acme/here": { "landing": "direct" },
            "acme/pr": { "landing": "pr", "testIn": "ci" },
            "acme/auto": { "testIn": "ci" },
        });
        let data = data.as_object().unwrap();
        let worked = |repo: &str| (repo == "acme/auto").then(|| "direct".to_string());
        let busy: Vec<(String, String)> =
            ["acme/ci", "acme/here", "acme/pr", "acme/auto", "ACME/ci"]
                .iter()
                .map(|r| (r.to_string(), "main".to_string()))
                .collect();
        assert_eq!(
            watched(&busy, data, &worked),
            vec![main_of("acme/ci"), main_of("acme/auto")]
        );
    }
}
