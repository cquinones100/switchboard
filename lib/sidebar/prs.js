// Part of bin/tmux-sidebar; see the comment at the top of that file.
//
// Whether each tab's pull request is in its merge queue, and where, or has
// merged, from GitHub: the tab's PR number is in its label (see model.js),
// and its repository is the origin of the tab's directory. Every PR_POLL_MS
// (PR_SLOW_MS with the poll_fast setting off) the tabs' PRs are asked about in one GraphQL query, through gh and its
// login, in the background; drawing reads only what the last answer said, so
// a redraw never waits on the network. A failed query keeps the last answers.
//
// A tab whose worktree has been deleted, as queue-and-clean does once its PR
// merges, has no branch left to read a PR from, so its label loses the PR. A
// worktree lives at <repo>/.claude/worktrees/<branch>, so such a tab's PR is
// asked for by that branch, in that repository, and the PR it finds is
// shown on the tab too.

const fs = require("node:fs");
const { countRun } = require("../requests");
const { repoOfUrl } = require("../repos");
const { execFile, execFileSync } = require("node:child_process");
const { isOn } = require("../settings");

const PR_POLL_MS = 15_000;
const PR_SLOW_MS = 60_000;

// "owner/name" of a directory's origin on GitHub, or null; asked once per
// directory, since a tab's directory rarely changes and each ask starts git.
const repos = new Map();
const repoOf = (dir) => {
  if (!dir) return null;
  if (!repos.has(dir)) {
    let repo = null;
    try {
      const url = execFileSync("git", ["-C", dir, "remote", "get-url", "origin"], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"], timeout: 2_000 }).trim();
      // github.com itself, not a host ending in it such as gist.github.com,
      // whose gists GitHub's repository API doesn't know.
      repo = repoOfUrl(url) ?? null;
    } catch {}
    repos.set(dir, repo);
  }
  return repos.get(dir);
};

// { repo, branch } for a tab sitting in a worktree that no longer exists.
const goneWorktree = (dir) => {
  const parts = dir && /^(.*)\/\.claude\/worktrees\/([^/]+)$/.exec(dir);
  if (!parts || fs.existsSync(dir)) return undefined;
  const repo = repoOf(parts[1]);
  return repo ? { repo, branch: parts[2] } : undefined;
};

// Whether a tab's name is about another ticket than its branch: a tab whose
// worktree merged and was removed, then used to shape a new ticket ("Shaping
// ABC-2: …"), is that work now, and the old branch's PR isn't its own, so it
// is neither in Merged nor closed with the merged tabs.
const TICKET_IN_NAME = /\b([A-Z][A-Z0-9]*-\d+)\b/;
const TICKET_IN_BRANCH = /^([a-z][a-z0-9]*-\d+)/i;
const elsewhere = (tab, branch) => {
  const named = TICKET_IN_NAME.exec(tab.name ?? "")?.[1];
  const ours = TICKET_IN_BRANCH.exec(branch ?? "")?.[1]?.toUpperCase();
  return Boolean(named && ours && named !== ours);
};

// "owner/name#123", or "owner/name@branch" for a gone worktree or a
// worktree no tab is in (the work map's, which come with their branch).
const keyOf = (tab) => {
  const repo = (tab.pr || tab.branch) && repoOf(tab.path);
  if (repo && tab.pr) return `${repo}#${tab.pr}`;
  if (repo && tab.branch) return elsewhere(tab, tab.branch) ? undefined : `${repo}@${tab.branch}`;
  const gone = goneWorktree(tab.path);
  return gone && !elsewhere(tab, gone.branch) ? `${gone.repo}@${gone.branch}` : undefined;
};

// key -> { merge: "queued" | "merged" | undefined, position, pr, prState (OPEN, CLOSED or MERGED), and the PR's
// checks ({ pass, fail, pending }), review decision, and size in lines }.
const known = new Map();

// What the tab shows: { merge, position }, and its PR when it was found by
// branch, from the last answer; or nothing.
// Nothing with the github setting off.
const mergeOf = (tab) => {
  if (!isOn("github")) return {};
  const key = keyOf(tab);
  return (key && known.get(key)) || {};
};

// One query for every PR the tabs name or their gone worktrees' branches
// lead to. Owners, names, numbers and branches go into the query text, so
// only what can be one is let in. Returns the query and, per alias, its key.
// What is asked about each PR: where it stands, and for the cards its
// checks counted by outcome (one count each, however many checks), its
// review and its size.
const PR_FIELDS = "number state reviewDecision additions deletions mergeQueueEntry { position } commits(last: 1) { nodes { commit { statusCheckRollup { contexts(first: 1) { checkRunCountsByState { state count } statusContextCountsByState { state count } } } } } }";
// GitHub's check states, in gh pr checks' buckets.
const BUCKET = { SUCCESS: "pass", NEUTRAL: "pass", COMPLETED: "pass", SKIPPED: "pass", FAILURE: "fail", ERROR: "fail", TIMED_OUT: "fail", CANCELLED: "fail", ACTION_REQUIRED: "fail", STARTUP_FAILURE: "fail", STALE: "fail" };
const checksOf = (pr) => {
  const contexts = pr.commits?.nodes?.[0]?.commit?.statusCheckRollup?.contexts;
  if (!contexts) return undefined;
  const counts = { pass: 0, fail: 0, pending: 0 };
  [...(contexts.checkRunCountsByState ?? []), ...(contexts.statusContextCountsByState ?? [])].forEach(({ state, count }) => (counts[BUCKET[state] ?? "pending"] += count));
  return counts;
};

const queryFor = (keys) => {
  const byRepo = new Map();
  keys.forEach((key) => {
    const at = /^([\w.-]+\/[\w.-]+)(?:#(\d+)|@([\w./-]+))$/.exec(key);
    if (!at) return;
    byRepo.set(at[1], [...(byRepo.get(at[1]) ?? []), { key, number: at[2], branch: at[3] }]);
  });
  const aliases = [];
  const parts = [...byRepo].map(([repo, wanted], i) => {
    const [owner, name] = repo.split("/");
    const asks = wanted.map((w, j) => {
      aliases.push({ repo: `r${i}`, alias: `a${j}`, key: w.key, byBranch: Boolean(w.branch) });
      return w.number
        ? `a${j}: pullRequest(number: ${w.number}) { ${PR_FIELDS} }`
        : `a${j}: pullRequests(headRefName: "${w.branch}", first: 1, orderBy: { field: CREATED_AT, direction: DESC }) { nodes { ${PR_FIELDS} } }`;
    });
    return `r${i}: repository(owner: "${owner}", name: "${name}") { ${asks.join(" ")} }`;
  });
  return { query: parts.length ? `query { ${parts.join(" ")} }` : undefined, aliases };
};

// Asks about the tabs' PRs, and calls `changed` when an answer differs from
// the last.
const fetchMerges = (tabs, changed, log = () => {}) => {
  if (!isOn("github")) return Promise.resolve();
  const keys = [...new Set(tabs.map(keyOf).filter(Boolean))];
  const { query, aliases } = queryFor(keys);
  if (!query) return Promise.resolve();
  return new Promise((resolve) => {
    countRun("gh", "GitHub", "GraphQL", "sidebar: tab PRs");
    execFile("gh", ["api", "graphql", "-f", `query=${query}`], { encoding: "utf8", timeout: 20_000 }, (err, stdout, stderr) => {
      // What GitHub said, not the command (the query, which says nothing).
      // One PR or repository it can't find fails the call but still answers
      // for the rest, so those are read all the same.
      if (err) log(`merge states: ${String(stderr || "").trim().split("\n")[0] || String(err.message).split("\n")[0]}`);
      let data;
      // An answer GitHub reported an error in (its path names the repository
      // and the PR's alias) is left as last known, not read as an answer.
      let failed;
      try {
        const answer = JSON.parse(stdout);
        data = answer.data ?? {};
        failed = new Set((answer.errors ?? []).map((e) => (e.path ?? []).slice(0, 2).join(".")));
      } catch {
        return resolve();
      }
      let differs = false;
      aliases.forEach(({ repo, alias, key, byBranch }) => {
        if (failed.has(repo) || failed.has(`${repo}.${alias}`)) return;
        const answer = data[repo]?.[alias];
        const pr = byBranch ? answer?.nodes?.[0] : answer;
        if (!pr) return;
        const checks = checksOf(pr);
        const next = {
          // Whether it is still open, for model.js's buildFailing.
          prState: pr.state,
          ...(pr.state === "MERGED" ? { merge: "merged" } : pr.mergeQueueEntry ? { merge: "queued", position: pr.mergeQueueEntry.position } : {}),
          ...(byBranch ? { pr: String(pr.number) } : {}),
          ...(checks && pr.state !== "MERGED" ? { checks } : {}),
          ...(pr.reviewDecision ? { review: pr.reviewDecision } : {}),
          ...(pr.additions !== undefined ? { size: { additions: pr.additions, deletions: pr.deletions } } : {}),
        };
        if (JSON.stringify(known.get(key) ?? {}) !== JSON.stringify(next)) differs = true;
        known.set(key, next);
      });
      if (differs) changed();
      resolve();
    });
  });
};

module.exports = { PR_POLL_MS, PR_SLOW_MS, PR_FIELDS, repoOf, mergeOf, queryFor, fetchMerges, known };
