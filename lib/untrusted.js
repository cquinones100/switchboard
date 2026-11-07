// Worktrees holding someone else's code, which `wtpr --untrusted` makes for a
// fork's PR: under REVIEW_DIR, with git's hooks off and `switchboard.untrusted`
// set in their own worktree config, which no file in the worktree can change.
// Nothing here starts Claude in one (its CLAUDE.md, settings, hooks and skills
// are the PR's) or lists one as work to reopen; its code runs only in
// `switchboard-try --sealed`.

const { spawnSync } = require("node:child_process");
const os = require("node:os");
const path = require("node:path");

const REVIEW_DIR = path.join(os.homedir(), ".cache/switchboard/pr-review");

// Whether a path is in the review folder, by its text alone (git prints a
// worktree's path as made), so a list can be filtered without a process.
const inReviewDir = (dir) => Boolean(dir) && (dir === REVIEW_DIR || dir.startsWith(`${REVIEW_DIR}/`));

// Whether dir is in an untrusted worktree: in the review folder, or marked so
// in its worktree config wherever it is.
const isUntrusted = (dir) => {
  if (inReviewDir(path.resolve(dir))) return true;
  const marked = spawnSync("git", ["-C", dir, "config", "--get", "switchboard.untrusted"], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] });
  return marked.status === 0 && marked.stdout.trim() === "true";
};

const refusal = (dir) =>
  `${dir} holds a PR wtpr --untrusted set up, whose CLAUDE.md, settings and hooks are its author's, so Claude isn't started there.\n` +
  "Review it from your main clone (/review-pr), and try its code with switchboard-try --sealed.";

module.exports = { REVIEW_DIR, inReviewDir, isUntrusted, refusal };
