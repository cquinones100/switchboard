// Part of bin/tmux-sidebar; see the comment at the top of that file.
//
// Mission control's Merge queues view: every pull request in the merge queue
// of each repository your tabs are in, everyone's, in queue order, the ones a
// tab of yours has open marked with that tab. Fetched every QUEUE_POLL_MS in
// the background, and when you go to the view, through gh: one GraphQL query
// for every repository's default branch's queue. Drawing reads only the last
// answer. Only while the github and view_merge_queues settings are on.

const { execFile } = require("node:child_process");
const { repoOf } = require("./prs");
const { NO_REPOS, githubCheck } = require("./setup-help");

const QUEUE_POLL_MS = 60_000;
const ENTRY = "position state enqueuedAt estimatedTimeToMerge pullRequest { number title url author { login } }";

// byRepo: { "owner/name": { url, entries } } for each repository with a
// merge queue, as GitHub last answered; the tabs are matched as it is drawn,
// so a tab opened or renumbered since shows at once.
const queues = { byRepo: {}, loaded: false, error: undefined };

// What GitHub calls an entry's state, in words.
const STATE_WORDS = { QUEUED: "queued", AWAITING_CHECKS: "awaiting checks", MERGEABLE: "ready to merge", UNMERGEABLE: "can't merge", LOCKED: "merging" };

// About how long until it merges, as GitHub estimates it in seconds.
const eta = (seconds) => (!seconds ? "" : seconds < 90 ? "merges in ~1m" : seconds < 5400 ? `merges in ~${Math.round(seconds / 60)}m` : `merges in ~${Math.round(seconds / 3600)}h`);

// The view's rows from GitHub's answer: { "owner/name": { url, entries } }
// per repository with a merge queue, and the tabs, so a tab's PR is marked.
const queueItems = (byRepo, tabs) =>
  Object.entries(byRepo).flatMap(([repo, queue]) =>
    [...queue.entries]
      .sort((a, b) => a.position - b.position)
      .map((entry) => {
        const pr = entry.pullRequest;
        const tab = tabs.find((t) => String(t.pr ?? "") === String(pr.number) && repoOf(t.path) === repo);
        return {
          title: `${entry.position}. #${pr.number} ${pr.title}`,
          detail: [repo.split("/")[1], STATE_WORDS[entry.state] ?? String(entry.state).toLowerCase(), eta(entry.estimatedTimeToMerge), pr.author?.login, tab && `tab ${tab.index}`].filter(Boolean).join(" · "),
          // A tab's PR that can't merge needs you; a tab's otherwise is yours to watch.
          state: tab ? (entry.state === "UNMERGEABLE" ? "attention" : "active") : "idle",
          time: Date.parse(entry.enqueuedAt) || undefined,
          url: pr.url,
          search: `${repo} ${pr.author?.login ?? ""}`,
          queue: { repo, number: pr.number, url: pr.url, queueUrl: queue.url, tab: tab?.id },
        };
      }),
  );

const fetchQueues = (tabs, changed, log = () => {}) => {
  const repos = [...new Set(tabs.map((t) => repoOf(t.path)).filter(Boolean))].filter((r) => /^[\w.-]+\/[\w.-]+$/.test(r)).sort();
  if (!repos.length) {
    Object.assign(queues, { loaded: true, error: NO_REPOS, byRepo: {} });
    changed();
    return Promise.resolve();
  }
  // Owners and names go into the query text, so only what can be one is let in.
  const parts = repos.map((repo, i) => {
    const [owner, name] = repo.split("/");
    return `r${i}: repository(owner: "${owner}", name: "${name}") { mergeQueue { url entries(first: 50) { nodes { ${ENTRY} } } } }`;
  });
  return new Promise((resolve) => {
    execFile("gh", ["api", "graphql", "-f", `query=query { ${parts.join(" ")} }`], { encoding: "utf8", timeout: 30_000 }, async (err, stdout, stderr) => {
      let data;
      let failed = new Set();
      try {
        const answer = JSON.parse(stdout);
        data = answer.data;
        // A repository GitHub reported an error for (its path names the
        // alias) keeps its last answer rather than dropping out of the view.
        failed = new Set((answer.errors ?? []).map((e) => e.path?.[0]).filter(Boolean));
      } catch {}
      queues.loaded = true;
      // One repository GitHub can't find fails the call but still answers
      // for the rest.
      if (!data) {
        queues.error = (await githubCheck()) ?? `Couldn't ask GitHub about the merge queues: ${String(stderr || err?.message || "").trim().split("\n")[0]}`;
        log(`merge queues: ${queues.error}`);
        changed();
        return resolve();
      }
      const byRepo = {};
      const unanswered = [];
      repos.forEach((repo, i) => {
        if (failed.has(`r${i}`)) {
          log(`merge queues: GitHub couldn't answer for ${repo}`);
          if (queues.byRepo[repo]) byRepo[repo] = queues.byRepo[repo];
          else unanswered.push(repo);
          return;
        }
        const queue = data[`r${i}`]?.mergeQueue;
        if (queue) byRepo[repo] = { url: queue.url, entries: (queue.entries?.nodes ?? []).filter((e) => e?.pullRequest) };
      });
      queues.byRepo = byRepo;
      queues.unanswered = unanswered;
      queues.error = undefined;
      changed();
      resolve();
    });
  });
};

// Empty, it says whose queues it looked at, or that none has one.
const queuesView = (tabs) => ({
  title: "Merge queues",
  queuesView: true,
  items: queueItems(queues.byRepo, tabs),
  loaded: queues.loaded,
  // A repository GitHub didn't answer for, with nothing from before, is said
  // in red above the list rather than taken as having no queue.
  error: queues.error ?? (queues.unanswered?.length ? `GitHub couldn't say what is in the merge queue of ${queues.unanswered.join(", ")}.` : undefined),
  builtIn: true,
  empty: Object.keys(queues.byRepo).length ? `Nothing in the merge queue of ${Object.keys(queues.byRepo).join(", ")}.` : "None of the repositories your tabs are in uses a merge queue.",
});

module.exports = { QUEUE_POLL_MS, fetchQueues, queuesView, queueItems, queues };
