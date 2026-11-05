// Part of bin/tmux-sidebar; see the comment at the top of that file.
//
// M-d's diff story told ahead of time: when a tab's branch has a commit
// whose story isn't told yet, tmux/diff-story-ahead.sh tells it in the
// background, so M-d opens on the story at once rather than waiting on
// Haiku. One branch at a time, each commit once. The branch's commit is read
// from git's files, not by starting git, since the daemon looks every
// minute. The default branch counts too, since commits not pushed yet are
// its diff (this repository's main, worked on straight); one with none is
// told in a moment, its diff being empty. Only with the diff_story and
// diff_story_ahead settings on, and github, which finds the PR's base.

const fs = require("node:fs");
const path = require("node:path");
const { execFile } = require("node:child_process");

const SCRIPT = path.join(__dirname, "../../tmux/diff-story-ahead.sh");

const read = (file) => {
  try {
    return fs.readFileSync(file, "utf8").trim();
  } catch {
    return null;
  }
};

// A directory's repository: its top, its branch and the branch's commit,
// from the files under .git; null outside one.
const headOf = (dir) => {
  let top = dir;
  while (top && !fs.existsSync(path.join(top, ".git"))) {
    const up = path.dirname(top);
    top = up === top ? null : up;
  }
  if (!top) return null;
  let gitdir = path.join(top, ".git");
  if (fs.statSync(gitdir).isFile()) {
    const pointer = read(gitdir)?.match(/^gitdir: (.+)$/)?.[1];
    if (!pointer) return null;
    gitdir = path.resolve(top, pointer);
  }
  const common = path.resolve(gitdir, read(path.join(gitdir, "commondir")) ?? ".");
  const ref = read(path.join(gitdir, "HEAD"))?.match(/^ref: (refs\/heads\/.+)$/)?.[1];
  if (!ref) return null;
  const packed = (name) => (read(path.join(common, "packed-refs")) ?? "").split("\n").find((line) => line.endsWith(` ${name}`))?.split(" ")[0];
  const sha = read(path.join(common, ref)) ?? packed(ref);
  return sha ? { top, branch: ref.slice("refs/heads/".length), sha } : null;
};

const told = new Map();
// A commit whose story couldn't be told (Claude down, say), with when: tried
// again only after RETRY_MS, so a lasting failure isn't asked every minute.
const failed = new Map();
const RETRY_MS = 30 * 60_000;
let running = false;

// Tells the story of the first tab's branch with a commit not told yet.
const storyAhead = (tabs, log = () => {}) => {
  if (running) return;
  const seen = new Set();
  const next = tabs
    .map((tab) => tab.path && headOf(tab.path))
    .filter((info) => info && !seen.has(info.top) && seen.add(info.top))
    .find((info) => told.get(info.top) !== info.sha && !(Date.now() - (failed.get(`${info.top} ${info.sha}`) ?? 0) < RETRY_MS));
  if (!next) return;
  running = true;
  execFile("bash", [SCRIPT, next.top], { timeout: 200_000 }, (err) => {
    running = false;
    if (!err) return told.set(next.top, next.sha);
    failed.set(`${next.top} ${next.sha}`, Date.now());
    log(`story ahead: ${next.branch}: not told (${String(err.message).split("\n")[0]}); trying again later`);
  });
};

module.exports = { headOf, storyAhead };
