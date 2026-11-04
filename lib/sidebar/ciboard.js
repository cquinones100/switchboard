// Part of bin/tmux-sidebar; see the comment at the top of that file.
//
// Mission control's CI view: your open pull requests whose checks are
// failing, in the repositories your tabs are in, each with the checks that
// failed and the line of its log that says why. Fetched every CI_POLL_MS in
// the background, and when you go to the view, through gh: one search for the
// pull requests and their checks, then the log of each failing GitHub Actions
// job, read once (a finished job's log doesn't change) and kept in
// STATE_DIR/ci-logs/<job>.log for "Read the failing log". Drawing reads only
// the last answer. Only while the github and view_ci settings are on.

const fs = require("node:fs");
const { countRun } = require("../requests");
const path = require("node:path");
const { execFile } = require("node:child_process");
const { STATE_DIR } = require("./config");
const { repoOf } = require("./prs");
const { NO_REPOS, githubCheck } = require("./setup-help");

const CI_POLL_MS = 120_000;
const MAX_LOGS = 15;
const LOGS = () => path.join(STATE_DIR, "ci-logs");
const FAILED = ["FAILURE", "TIMED_OUT", "ACTION_REQUIRED", "STARTUP_FAILURE", "ERROR"];
const FIELDS = `... on PullRequest { number title url headRefName repository { nameWithOwner }
  commits(last: 1) { nodes { commit { statusCheckRollup { state contexts(first: 100) { nodes {
    __typename ... on CheckRun { name conclusion detailsUrl } ... on StatusContext { context state targetUrl }
  } } } } } } }`;

const ci = { items: [], loaded: false, error: undefined };

const gh = (args) =>
  new Promise((resolve) => {
    countRun("gh", "GitHub", args[1] === "graphql" ? "GraphQL" : "REST", "sidebar: CI");
    execFile("gh", args, { encoding: "utf8", timeout: 30_000, maxBuffer: 64 * 1024 * 1024 }, (err, stdout) => resolve(err ? undefined : stdout));
  });

// The failing checks of a pull request: { name, url, job } each, job the
// GitHub Actions job's ID when it is one.
const failingOf = (pr) =>
  (pr.commits?.nodes?.[0]?.commit?.statusCheckRollup?.contexts?.nodes ?? [])
    .filter((c) => FAILED.includes(String(c.conclusion ?? c.state ?? "").toUpperCase()))
    .map((c) => {
      const url = c.detailsUrl ?? c.targetUrl;
      return { name: c.name ?? c.context ?? "check", url, job: /\/actions\/runs\/\d+\/job\/(\d+)/.exec(url ?? "")?.[1] };
    });

// The line of a log that says why it failed: the last that names an error or
// a failure as a word, leaving out the lines that only say a command failed,
// else its last line, without the timestamp Actions puts first.
const whyFrom = (log) => {
  const lines = log.replace(/\x1b\[[0-9;]*[A-Za-z]/g, "").split("\n").map((l) => l.replace(/^\d{4}-\d\d-\d\dT[\d:.]+Z\s?/, "").replace(/##\[(error|group|endgroup)\]/g, "").trim()).filter(Boolean);
  // The runner's and package managers' own last words only say it failed.
  const noise = /Process completed with exit code|ELIFECYCLE|Command failed with exit code|npm ERR! (A complete log|Lifecycle)|DeprecationWarning|^shell: /;
  // A line that says so the way tools do first (FAIL, Error:, ✗, a
  // traceback), then any that mentions an error or a failure.
  const strong = /\bFAIL(ED)?\b|\b[A-Za-z]*Error:|^error\b|✗|Traceback|panicked at/;
  const weak = /\b(errors?|fail(ed|ure|s)?)\b|assertion|exception/i;
  const last = (re) => [...lines].reverse().find((l) => re.test(l) && !noise.test(l));
  return last(strong) ?? last(weak) ?? lines.at(-1) ?? "";
};

const logFile = (job) => path.join(LOGS(), `${job}.log`);
const readLog = async (repo, job) => {
  if (fs.existsSync(logFile(job))) return fs.readFileSync(logFile(job), "utf8");
  // Logs carry color codes, which gh prints only when told to; less -R
  // shows them, and whyFrom drops them.
  const log = await gh(["api", "--allow-escape-sequences", `repos/${repo}/actions/jobs/${job}/logs`]);
  if (log === undefined) return undefined;
  fs.mkdirSync(LOGS(), { recursive: true });
  fs.writeFileSync(logFile(job), log);
  return log;
};

const fetchCi = async (tabs, changed, log = () => {}) => {
  const repos = [...new Set(tabs.map((t) => repoOf(t.path)).filter(Boolean))].filter((r) => /^[\w.-]+\/[\w.-]+$/.test(r));
  if (!repos.length) {
    Object.assign(ci, { loaded: true, error: NO_REPOS, items: [] });
    return changed();
  }
  const query = `query($q: String!) { search(query: $q, type: ISSUE, first: 50) { nodes { ${FIELDS} } } }`;
  const out = await gh(["api", "graphql", "-f", `query=${query}`, "-f", `q=is:pr is:open author:@me ${repos.map((r) => `repo:${r}`).join(" ")}`]);
  ci.loaded = true;
  if (out === undefined) {
    ci.error = (await githubCheck()) ?? "Couldn't ask GitHub about your pull requests' checks (gh).";
    log(`ci: ${ci.error}`);
    return changed();
  }
  let prs;
  try {
    prs = JSON.parse(out).data.search.nodes.filter((pr) => pr?.number);
  } catch (err) {
    ci.error = err.message;
    return changed();
  }
  let logs = 0;
  const items = [];
  for (const pr of prs) {
    const failing = failingOf(pr);
    if (!failing.length) continue;
    const repo = pr.repository.nameWithOwner;
    // The first failing job's log says why, read at most MAX_LOGS a look.
    const first = failing.find((f) => f.job);
    const text = first && logs++ < MAX_LOGS ? await readLog(repo, first.job) : undefined;
    items.push({
      title: `#${pr.number} ${pr.title}`,
      detail: [`✗ ${failing.map((f) => f.name).join(", ")}`, text && whyFrom(text)].filter(Boolean).join(" · "),
      state: "attention",
      url: pr.url,
      search: repo,
      ci: { number: pr.number, repo, url: pr.url, branch: pr.headRefName, check: failing[0], job: first?.job },
    });
  }
  ci.items = items;
  ci.error = undefined;
  // Logs of jobs no longer failing go.
  const keep = new Set(items.map((i) => i.ci.job).filter(Boolean).map((job) => `${job}.log`));
  try {
    fs.readdirSync(LOGS()).filter((name) => !keep.has(name)).forEach((name) => fs.rmSync(path.join(LOGS(), name), { force: true }));
  } catch {}
  changed();
};

const ciView = () => ({ title: "CI", ciView: true, items: ci.items, loaded: ci.loaded, error: ci.error, builtIn: true });

module.exports = { CI_POLL_MS, fetchCi, ciView, ci, failingOf, whyFrom, logFile };
