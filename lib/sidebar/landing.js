// Part of bin/tmux-sidebar; see the comment at the top of that file.
//
// The landing queue of queue-and-clean's "Land on the default branch"
// (claude/skills/queue-and-clean/scripts/land-lock.js): which tab's worktree
// holds the landing lock and which wait for it, in order, read from the files
// the script keeps in the main clone's git directory. Found from files alone,
// with no git process, since this is the drawing path: a worktree's `.git`
// file names its git directory, whose `commondir` leads to the shared one.

const fs = require("node:fs");
const path = require("node:path");
const { isOn } = require("../settings");
const { sessions } = require("./claude");

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
const dirsOf = (tab) => [tab.claude?.cwd, tab.path, ...(tab.panes ?? []).filter((p) => p["@notes_pane"] !== "1").map((p) => p.pane_current_path)].filter(Boolean);

// Each tab's place in its repository's landing queue, set on the tab as
// `landing`: { holding: true, line: 1 } for the worktree landing, and
// { line: n, root } for the nth in line, each waiter's worktree `root` for
// moving it, from the first of its directories in a git repository. Each
// repository's files are read once a refresh. The
// landing_queue setting off leaves the tabs as they were.
const landingOf = (tabs) => {
  if (!isOn("landing_queue")) return;
  const repos = new Map();
  let running;
  tabs.forEach((tab) => {
    const git = dirsOf(tab).map(gitOf).find(Boolean);
    if (!git) return;
    if (!repos.has(git.common)) {
      running ??= sessions().filter((session) => typeof session.cwd === "string" && alive(session.pid));
      const live = (where) => typeof where === "string" && liveIn(where, running);
      const holder = readJson(path.join(git.common, "land.lock/holder"))?.path;
      const queue = readJson(path.join(git.common, "land.queue"));
      repos.set(git.common, { holder: live(holder) ? holder : undefined, queue: (Array.isArray(queue) ? queue : []).map((entry) => entry?.path).filter(live) });
    }
    const { holder, queue } = repos.get(git.common);
    const at = queue.indexOf(git.root);
    if (holder === git.root) tab.landing = { holding: true, line: 1 };
    else if (at !== -1) tab.landing = { line: at + (holder ? 2 : 1), root: git.root, first: at === 0, last: at === queue.length - 1 };
  });
};

const ordinal = (n) => `${n}${n % 100 >= 11 && n % 100 <= 13 ? "th" : ["th", "st", "nd", "rd"][n % 10] ?? "th"}`;
const landingWords = (tab) => (!tab.landing ? "" : tab.landing.holding ? "landing" : `${ordinal(tab.landing.line)} in line to land`);

module.exports = { LAND_LOCK, gitOf, dirsOf, landingOf, landingWords };
