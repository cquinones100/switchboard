#!/usr/bin/env node
// Waits until a merged PR's commit has been deployed, and prints one line:
//
//   deployed <sha> <url>   a deploy that includes the commit succeeded (url may be "-")
//   failed <url>           every finished deploy that includes it failed
//   timeout                nothing settled within --timeout minutes
//
//   node wait-deploy.js --repo <owner/name> --sha <merge commit>
//     (--environment <name> [--ref <branch>] | --workflow <name> --branch <base>) [--timeout 45]
//
// --environment watches GitHub deployments to that environment (what Vercel,
// Netlify, Render and most deploy actions report), only those of one branch
// with --ref (a PR's preview, where every PR shares the environment), or of
// the commit itself; --workflow watches runs of
// a deploy workflow on the base branch. A deploy counts when its commit is the
// merge commit or comes after it, since a merge queue or a quick second merge
// often deploys a later commit that includes this one. A failed request is
// retried on the next round rather than read as an answer. Plain Node, gh for
// GitHub, no packages.

const { execFileSync } = require("node:child_process");

const args = process.argv.slice(2);
const arg = (name) => (args.includes(`--${name}`) ? args[args.indexOf(`--${name}`) + 1] : undefined);
const repo = arg("repo");
const sha = arg("sha");
const environment = arg("environment");
const workflow = arg("workflow");
const branch = arg("branch");
const ref = arg("ref");
const timeout = Number(arg("timeout") ?? 45) * 60 * 1000;
if (!repo || !sha || !(environment || (workflow && branch))) {
  console.error("usage: node wait-deploy.js --repo <owner/name> --sha <sha> (--environment <name> [--ref <branch>] | --workflow <name> --branch <base>) [--timeout 45]");
  process.exit(2);
}

const gh = (...ghArgs) => JSON.parse(execFileSync("gh", ghArgs, { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }));

// Whether a deployed commit includes the merge commit, remembered per commit.
const included = new Map();
const includes = (deployed) => {
  if (!included.has(deployed)) {
    const { status } = gh("api", `repos/${repo}/compare/${sha}...${deployed}`);
    included.set(deployed, status === "identical" || status === "ahead");
  }
  return included.get(deployed);
};

// With --ref, a deployment recorded under the branch's name or under the
// commit itself, as some deploy actions record it, newest first.
const fromDeployments = () => {
  const list = (forRef) => gh("api", `repos/${repo}/deployments?environment=${encodeURIComponent(environment)}${forRef ? `&ref=${encodeURIComponent(forRef)}` : ""}&per_page=15`);
  const found = ref ? [...list(ref), ...list(sha)] : list();
  const deployments = found
    .filter((deployment, i) => found.findIndex((other) => other.id === deployment.id) === i)
    .sort((a, b) => b.id - a.id)
    .filter((deployment) => includes(deployment.sha))
    .map((deployment) => ({ deployment, status: gh("api", `repos/${repo}/deployments/${deployment.id}/statuses?per_page=1`)[0] }));
  const done = deployments.find(({ status }) => status?.state === "success");
  if (done) return `deployed ${done.deployment.sha} ${done.status.environment_url || "-"}`;
  const pending = deployments.some(({ status }) => !status || ["queued", "pending", "in_progress"].includes(status.state));
  const failed = deployments.find(({ status }) => ["failure", "error"].includes(status?.state));
  return failed && !pending ? `failed ${failed.status.log_url || failed.status.target_url || "-"}` : null;
};

const fromWorkflow = () => {
  const runs = gh("run", "list", "-R", repo, "--workflow", workflow, "--branch", branch, "--limit", "15", "--json", "headSha,status,conclusion,url")
    .filter((run) => includes(run.headSha));
  const done = runs.find((run) => run.conclusion === "success");
  if (done) return `deployed ${done.headSha} -`;
  const pending = runs.some((run) => run.status !== "completed");
  const failed = runs.find((run) => ["failure", "timed_out", "startup_failure"].includes(run.conclusion));
  return failed && !pending ? `failed ${failed.url}` : null;
};

const started = Date.now();
const poll = () => {
  try {
    const answer = environment ? fromDeployments() : fromWorkflow();
    if (answer) {
      console.log(answer);
      return;
    }
  } catch {
    // A network blip or a rate limit: try again next round.
  }
  if (Date.now() - started > timeout) {
    console.log("timeout");
    return;
  }
  setTimeout(poll, 30_000);
};
poll();
