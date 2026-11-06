//! `wait-deploy`: waits until a merged PR's commit has been deployed, and
//! prints one line. Ported from claude/skills/deploy-test/scripts/wait-deploy.js.
//!
//! ```text
//! deployed <sha> <url>   a deploy that includes the commit succeeded (url may be "-")
//! failed <url>           every finished deploy that includes it failed
//! timeout                nothing settled within --timeout minutes
//!
//! wait-deploy --repo <owner/name> --sha <merge commit>
//!   (--environment <name> [--ref <branch>] | --workflow <name> --branch <base>) [--timeout 45]
//! ```
//!
//! `--environment` watches GitHub deployments to that environment (what
//! Vercel, Netlify, Render and most deploy actions report), only those of one
//! branch with `--ref` (a PR's preview, where every PR shares the
//! environment), or of the commit itself; `--workflow` watches runs of a
//! deploy workflow on the base branch. A deploy counts when its commit is the
//! merge commit or comes after it, since a merge queue or a quick second
//! merge often deploys a later commit that includes this one. A failed
//! request is retried on the next round rather than read as an answer.
//!
//! GitHub is asked over its REST API (src/integrations/github.rs) where the
//! Node version ran `gh api` and `gh run list`. `gh run list --workflow` took
//! a workflow's name or its file; the API takes the file name or its id, so
//! a name it doesn't know is looked up among the repository's workflows by
//! the name they show. It looks every 30 seconds, or every
//! `WAIT_DEPLOY_POLL_SECS`, for the tests.

use std::collections::HashMap;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use serde_json::Value;

use crate::integrations::github;
use crate::net::{self, ErrorKind};
use crate::util;

const POLL: Duration = Duration::from_secs(30);
const SOURCE: &str = "wait-deploy";
const USAGE: &str = "usage: wait-deploy --repo <owner/name> --sha <sha> (--environment <name> [--ref <branch>] | --workflow <name> --branch <base>) [--timeout 45]";

/// What it was asked to wait for.
struct Watch {
    repo: String,
    sha: String,
    environment: Option<String>,
    workflow: Option<String>,
    branch: Option<String>,
    git_ref: Option<String>,
    /// Whether each deployed commit includes the merge commit, remembered.
    included: HashMap<String, bool>,
    /// The workflow's id, once a name had to be looked up.
    workflow_id: Option<String>,
}

pub fn main(args: Vec<String>) -> ExitCode {
    let arg = |name: &str| {
        let flag = format!("--{name}");
        args.iter()
            .position(|a| *a == flag)
            .and_then(|at| args.get(at + 1))
            .cloned()
    };
    let timeout_minutes = arg("timeout")
        .and_then(|t| t.trim().parse::<f64>().ok())
        .filter(|t| t.is_finite() && *t >= 0.0)
        .unwrap_or(45.0);
    let mut watch = Watch {
        repo: arg("repo").unwrap_or_default(),
        sha: arg("sha").unwrap_or_default(),
        environment: arg("environment"),
        workflow: arg("workflow"),
        branch: arg("branch"),
        git_ref: arg("ref"),
        included: HashMap::new(),
        workflow_id: None,
    };
    let by_workflow = watch.workflow.is_some() && watch.branch.is_some();
    if watch.repo.is_empty()
        || watch.sha.is_empty()
        || !(watch.environment.is_some() || by_workflow)
    {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    let poll = std::env::var("WAIT_DEPLOY_POLL_SECS")
        .ok()
        .and_then(|p| p.trim().parse::<f64>().ok())
        .filter(|p| p.is_finite() && *p >= 0.0)
        .map(Duration::from_secs_f64)
        .unwrap_or(POLL);
    let timeout = Duration::from_secs_f64(timeout_minutes * 60.0);
    let started = Instant::now();
    loop {
        // A network blip or a rate limit: try again next round.
        if let Ok(Some(answer)) = util::block_on(watch.look()) {
            println!("{answer}");
            return ExitCode::SUCCESS;
        }
        if started.elapsed() > timeout {
            println!("timeout");
            return ExitCode::SUCCESS;
        }
        std::thread::sleep(poll);
    }
}

/// A query parameter's value, percent-encoded as `encodeURIComponent` does.
fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

impl Watch {
    async fn look(&mut self) -> Result<Option<String>> {
        if self.environment.is_some() {
            self.look_at_deployments().await
        } else {
            self.look_at_workflow().await
        }
    }

    /// Whether a deployed commit includes the merge commit: it is the
    /// commit, or comes after it.
    async fn includes(&mut self, deployed: &str) -> Result<bool> {
        if let Some(known) = self.included.get(deployed) {
            return Ok(*known);
        }
        let compare = github::rest(
            "GET",
            &format!("repos/{}/compare/{}...{deployed}", self.repo, self.sha),
            None,
            SOURCE,
        )
        .await?;
        let status = text(&compare, "status");
        let yes = status == "identical" || status == "ahead";
        self.included.insert(deployed.to_string(), yes);
        Ok(yes)
    }

    async fn deployments(&self, environment: &str, for_ref: Option<&str>) -> Result<Vec<Value>> {
        let mut path = format!(
            "repos/{}/deployments?environment={}",
            self.repo,
            encode(environment)
        );
        if let Some(r) = for_ref {
            path.push_str(&format!("&ref={}", encode(r)));
        }
        path.push_str("&per_page=15");
        let list = github::rest("GET", &path, None, SOURCE).await?;
        Ok(list.as_array().cloned().unwrap_or_default())
    }

    /// With `--ref`, a deployment recorded under the branch's name or under
    /// the commit itself, as some deploy actions record it, newest first.
    async fn look_at_deployments(&mut self) -> Result<Option<String>> {
        let environment = self.environment.clone().unwrap_or_default();
        let found = match self.git_ref.clone() {
            Some(r) => {
                let mut all = self.deployments(&environment, Some(&r)).await?;
                let sha = self.sha.clone();
                all.extend(self.deployments(&environment, Some(&sha)).await?);
                all
            }
            None => self.deployments(&environment, None).await?,
        };
        let id = |d: &Value| d.get("id").and_then(Value::as_i64).unwrap_or(0);
        let mut unique: Vec<Value> = Vec::new();
        for d in found {
            if !unique.iter().any(|u| id(u) == id(&d)) {
                unique.push(d);
            }
        }
        unique.sort_by_key(|d| std::cmp::Reverse(id(d)));
        let mut deployments = Vec::new();
        for d in unique {
            if !self.includes(text(&d, "sha")).await? {
                continue;
            }
            let statuses = github::rest(
                "GET",
                &format!(
                    "repos/{}/deployments/{}/statuses?per_page=1",
                    self.repo,
                    id(&d)
                ),
                None,
                SOURCE,
            )
            .await?;
            let status = statuses.get(0).cloned();
            deployments.push((d, status));
        }
        let state = |s: &Option<Value>| s.as_ref().map(|s| text(s, "state").to_string());
        if let Some((d, Some(status))) = deployments
            .iter()
            .find(|(_, s)| state(s).as_deref() == Some("success"))
        {
            let url = text(status, "environment_url");
            return Ok(Some(format!(
                "deployed {} {}",
                text(d, "sha"),
                if url.is_empty() { "-" } else { url }
            )));
        }
        let pending = deployments.iter().any(|(_, s)| {
            s.is_none()
                || matches!(
                    state(s).as_deref(),
                    Some("queued" | "pending" | "in_progress")
                )
        });
        let failed = deployments
            .iter()
            .find(|(_, s)| matches!(state(s).as_deref(), Some("failure" | "error")));
        Ok(match failed {
            Some((_, Some(status))) if !pending => {
                let url = [text(status, "log_url"), text(status, "target_url")]
                    .into_iter()
                    .find(|u| !u.is_empty())
                    .unwrap_or("-");
                Some(format!("failed {url}"))
            }
            _ => None,
        })
    }

    async fn workflow_runs(&mut self, workflow: &str) -> Result<Value> {
        let branch = self.branch.clone().unwrap_or_default();
        github::rest(
            "GET",
            &format!(
                "repos/{}/actions/workflows/{}/runs?branch={}&per_page=15",
                self.repo,
                encode(workflow),
                encode(&branch)
            ),
            None,
            SOURCE,
        )
        .await
    }

    /// The runs of the deploy workflow on the base branch: by the file name
    /// or id given, else by the name a workflow shows.
    async fn runs(&mut self) -> Result<Vec<Value>> {
        let given = self.workflow.clone().unwrap_or_default();
        let workflow = self.workflow_id.clone().unwrap_or_else(|| given.clone());
        let answer = match self.workflow_runs(&workflow).await {
            Err(e)
                if net::kind_of(&e) == Some(&ErrorKind::NotFound) && self.workflow_id.is_none() =>
            {
                let all = github::rest(
                    "GET",
                    &format!("repos/{}/actions/workflows?per_page=100", self.repo),
                    None,
                    SOURCE,
                )
                .await?;
                let found = all
                    .get("workflows")
                    .and_then(Value::as_array)
                    .and_then(|list| list.iter().find(|w| text(w, "name") == given))
                    .and_then(|w| w.get("id"))
                    .map(util::js_string)
                    .ok_or_else(|| anyhow!("{} has no workflow named {given}", self.repo))?;
                self.workflow_id = Some(found.clone());
                self.workflow_runs(&found).await?
            }
            other => other?,
        };
        Ok(answer
            .get("workflow_runs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    async fn look_at_workflow(&mut self) -> Result<Option<String>> {
        let mut runs = Vec::new();
        for run in self.runs().await? {
            if self.includes(text(&run, "head_sha")).await? {
                runs.push(run);
            }
        }
        if let Some(done) = runs.iter().find(|r| text(r, "conclusion") == "success") {
            return Ok(Some(format!("deployed {} -", text(done, "head_sha"))));
        }
        let pending = runs.iter().any(|r| text(r, "status") != "completed");
        let failed = runs.iter().find(|r| {
            matches!(
                text(r, "conclusion"),
                "failure" | "timed_out" | "startup_failure"
            )
        });
        Ok(match failed {
            Some(run) if !pending => Some(format!("failed {}", text(run, "html_url"))),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_values_are_encoded_as_encode_uri_component_does() {
        assert_eq!(
            encode("Preview – web/main"),
            "Preview%20%E2%80%93%20web%2Fmain"
        );
        assert_eq!(encode("prod_1.x"), "prod_1.x");
    }
}
