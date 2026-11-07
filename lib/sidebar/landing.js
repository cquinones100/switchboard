// Part of bin/tmux-sidebar; see the comment at the top of that file.
//
// What is landing in each repository a tab is in: queue-and-clean's "Land on
// the default branch" (claude/skills/queue-and-clean/scripts/land-lock.js),
// which worktree holds the landing lock and which wait for it, in order, read
// from the files the script keeps in the main clone's git directory; and its
// merge queue, as queues.js last heard it. Found from files alone, with no
// git process but the one asking a directory's origin once (prs.js's
// repoOf), since this is the drawing path: a worktree's `.git`
// file names its git directory, whose `commondir` leads to the shared one.

const fs = require("node:fs");
const path = require("node:path");
const { isOn } = require("../settings");
const { sessions } = require("./claude");
const { repoOf } = require("./prs");
const { queueItems } = require("./queues");

const LAND_LOCK = path.join(__dirname, "../../claude/skills/queue-and-clean/scripts/land-lock.js");

// A directory's worktree and its repository's common git directory:
// { root, common }, or undefined outside a git repository.
const gitOf = (dir) => {
  for (let at = dir; at; at = path.dirname(at) === at ? undefined : path.dirname(at)) {
    const dotGit = path.join(at, ".git");
    let stat;
    try {
      stat = fs.statSync(dotGit);
    } catch {
      continue;
    }
    try {
      const root = fs.realpathSync(at);
      if (stat.isDirectory()) return { root, common: fs.realpathSync(dotGit) };
      const gitdir = path.resolve(at, /^gitdir: (.+)$/m.exec(fs.readFileSync(dotGit, "utf8"))[1].trim());
      // A submodule or a --separate-git-dir clone has no commondir: its git
      // directory is the shared one.
      const commondir = fs.existsSync(path.join(gitdir, "commondir")) ? fs.readFileSync(path.join(gitdir, "commondir"), "utf8").trim() : ".";
      return { root, common: fs.realpathSync(path.resolve(gitdir, commondir)) };
    } catch {
      return undefined;
    }
  }
  return undefined;
};

const readJson = (file) => {
  try {
    return JSON.parse(fs.readFileSync(file, "utf8"));
  } catch {
    return undefined;
  }
};

const alive = (pid) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch (err) {
    return err.code === "EPERM";
  }
};

// A holder or waiter counts while its worktree is there and a Claude session
// runs in it, as land-lock.js's gone() has it, so a landing that ended
// without a word leaves the group before the script next reads the queue.
// From the session files and a signal 0, starting no process.
// A session in a worktree nested inside another (the main clone's
// .claude/worktrees) is that one's, not the outer one's.
const liveIn = (root, running) => fs.existsSync(root) && running.some((session) => (session.cwd === root || session.cwd.startsWith(`${root}/`)) && gitOf(session.cwd)?.root === root);

// Where a tab works: its Claude session's directory, else its panes' (Notes,
// which M-e may have put beside it and focused, runs in your home directory).
// The tab's own path is its active pane's, left out when that is Notes.
const dirsOf = (tab) => {
  const notesInFront = (tab.panes ?? []).some((p) => p["@notes_pane"] === "1" && p.pane_active === "1");
  return [tab.claude?.cwd, notesInFront ? undefined : tab.path, ...(tab.panes ?? []).filter((p) => p["@notes_pane"] !== "1").map((p) => p.pane_current_path)].filter(Boolean);
};

// Each repository a tab is in that has something landing, in the order
// its tabs first come: { key, repo, name, branch, direct, queue }. direct is
// the landing lock's { holder, waiting }, each entry { path, since }, live
// entries only, a worktree no tab is in kept by its path; queue is its merge
// queue as GitHub last answered (queues.js's byRepo: { url, entries }), every
// PR in it, everyone's. A repository with neither is left out. Read from
// files, the repository's name from prs.js's repoOf, asked once a directory.
const landingRepos = (tabs, byRepo = {}) => {
  const repos = new Map();
  let running;
  tabs.forEach((tab) => {
    const git = dirsOf(tab).map(gitOf).find(Boolean);
    if (!git || repos.has(git.common)) return;
    running ??= sessions().filter((session) => typeof session.cwd === "string" && alive(session.pid));
    const live = (entry) => typeof entry?.path === "string" && liveIn(entry.path, running);
    const holder = readJson(path.join(git.common, "land.lock/holder"));
    const queue = readJson(path.join(git.common, "land.queue"));
    const repo = repoOf(git.root) ?? undefined;
    // origin/HEAD names the default branch, when the clone has it.
    let branch;
    try {
      branch = /^ref: refs\/remotes\/origin\/(.+)$/m.exec(fs.readFileSync(path.join(git.common, "refs/remotes/origin/HEAD"), "utf8"))?.[1]?.trim();
    } catch {}
    repos.set(git.common, {
      key: git.common,
      repo,
      name: repo ?? path.basename(git.common.replace(/\/\.git$/, "")),
      branch,
      direct: { holder: live(holder) ? { path: holder.path, since: holder.since } : undefined, waiting: (Array.isArray(queue) ? queue : []).filter(live).map((e) => ({ path: e.path, since: e.since })) },
      // A merge queue is the GitHub repository's, so two clones of it show
      // it once, with the first.
      queue: repo && ![...repos.values()].some((r) => r.repo === repo) ? byRepo[repo] : undefined,
    });
  });
  return [...repos.values()].filter((r) => r.direct.holder || r.direct.waiting.length || r.queue?.entries?.length);
};

// The repositories with something landing (landingRepos), each tab's place
// set on it as `landing` when the landing_queue setting is on: landing on
// the default branch, { method: "direct", holding: true, line: 1 } for the
// worktree holding the landing lock and { method: "direct", line: n, root,
// first, last } for the nth in line, each waiter's worktree `root` for moving
// it; its PR in the merge queue, { method: "pr", line: <its place>, holding:
// <first> }, from GitHub's last answer, else the tab's own (prs.js).
const landingOf = (tabs, byRepo = {}) => {
  const repos = landingRepos(tabs, byRepo);
  tabs.forEach((tab) => delete tab.landing);
  if (!isOn("landing_queue")) return repos;
  tabs.forEach((tab) => {
    const git = dirsOf(tab).map(gitOf).find(Boolean);
    const found = git && repos.find((r) => r.key === git.common);
    const { holder, waiting } = found?.direct ?? { waiting: [] };
    const at = git ? waiting.findIndex((e) => e.path === git.root) : -1;
    if (git && holder?.path === git.root) tab.landing = { method: "direct", holding: true, line: 1 };
    else if (at !== -1) tab.landing = { method: "direct", line: at + (holder ? 2 : 1), root: git.root, first: at === 0, last: at === waiting.length - 1 };
    else {
      const repo = repoOf(git?.root);
      // A PR GitHub has since said is merged has left the queue, whatever the
      // queue said last.
      const entry = tab.pr && repo && tab.merge !== "merged" && byRepo[repo]?.entries?.find((e) => String(e.pullRequest?.number) === String(tab.pr));
      const line = entry?.position ?? (tab.merge === "queued" ? tab.position : undefined);
      if (line) tab.landing = { method: "pr", line, holding: line === 1 };
    }
  });
  return repos;
};

// Mission control's Landing view, shown only while something lands
// (landingRepos), while it is in front (`keep`), so it doesn't vanish
// under you, or while the merge queues can't be asked of GitHub (`error`): a section per repository and way, headed "<repo> · straight to
// <branch>" or "<repo> · merge queue", its rows in queue order. A direct row
// is the tab landing or waiting (the worktree's folder when no tab has it),
// with its step and how long since it took the lock or joined; a merge
// queue's are queues.js's rows. Enter's menus are the daemon's itemChoices.
const landingView = (tabs, repos, { keep = false, error } = {}) => {
  // Nor while GitHub can't be asked: then it says why, as a view needing
  // setup does.
  if (!repos.length && !keep && !error) return undefined;
  const rootOf = (tab) => dirsOf(tab).map(gitOf).find(Boolean)?.root;
  const items = repos.flatMap((r) => {
    const { holder, waiting } = r.direct;
    const direct = [...(holder ? [{ ...holder, holding: true }] : []), ...waiting];
    const rows = direct.map((entry, i) => {
      const tab = tabs.find((t) => rootOf(t) === entry.path);
      const at = i - (holder ? 1 : 0);
      return {
        title: `${i + 1}. ${tab ? tab.name : path.basename(entry.path)}`,
        detail: [entry.holding ? ["landing", tab?.phase].filter(Boolean).join(" · ") : "waiting", tab ? `tab ${tab.index}` : "no tab"].join(" · "),
        state: tab?.state === "waiting" ? "attention" : entry.holding ? "active" : "idle",
        time: Number(entry.since) || undefined,
        search: `${r.name} ${entry.path}`,
        landing: { root: entry.path, tab: tab?.id, holding: Boolean(entry.holding), first: at === 0, last: at === waiting.length - 1 },
      };
    });
    const queued = r.queue?.entries?.length ? queueItems({ [r.repo]: r.queue }, tabs) : [];
    const header = (how, count) => ({ title: `${r.name} · ${how}`, group: "category", count, state: "idle", detail: "" });
    return [...(rows.length ? [header(`straight to ${r.branch ?? "the default branch"}`, rows.length), ...rows] : []), ...(queued.length ? [header("merge queue", queued.length), ...queued] : [])];
  });
  return { title: "Landing", landing: true, queuesView: true, items, loaded: true, builtIn: true, error, empty: "Nothing is landing now." };
};

const ordinal = (n) => `${n}${n % 100 >= 11 && n % 100 <= 13 ? "th" : ["th", "st", "nd", "rd"][n % 10] ?? "th"}`;
const landingWords = (tab) => {
  if (!tab.landing) return "";
  if (tab.landing.method === "pr") return tab.landing.holding ? "merging" : `${ordinal(tab.landing.line)} in line to merge`;
  return tab.landing.holding ? "landing" : `${ordinal(tab.landing.line)} in line to land`;
};

// The worktree a tab works in (its Claude's, else its panes' but Notes'),
// or undefined outside a git repository.
const workTreeOf = (tab) => dirsOf(tab).map(gitOf).find(Boolean)?.root;

module.exports = { LAND_LOCK, gitOf, dirsOf, workTreeOf, landingRepos, landingOf, landingView, landingWords, ordinal };
