// Tests for tmux/pick-dir.zsh, the directory question of M-t and the worktree
// popups, run with: node --test test/*.test.js
//
// It runs against a throwaway tmux server with a temporary HOME and a stand-in
// fzf that keeps the list it was shown and takes its first row, so nothing
// reaches your tmux.

const { test, after } = require("node:test");
const assert = require("node:assert/strict");
const { execFileSync, spawnSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "pick-dir-")));
const socket = path.join(tmp, "tmux.sock");
const bin = path.join(tmp, "bin");
const repo = path.join(tmp, "repo");
const plain = path.join(tmp, "plain");
const worktree = path.join(repo, ".claude/worktrees/feature");
[bin, path.join(repo, "src"), plain].forEach((dir) => fs.mkdirSync(dir, { recursive: true }));
const env = { ...process.env, HOME: tmp, PATH: `${bin}:${process.env.PATH}`, TMUX: "", GIT_CONFIG_GLOBAL: "/dev/null", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e" };
execFileSync("git", ["init", "-q", "-b", "main", repo], { env });
execFileSync("git", ["-C", repo, "commit", "-q", "--allow-empty", "-m", "start"], { env });
execFileSync("git", ["-C", repo, "worktree", "add", "-q", "-b", "feature", worktree], { env });
fs.writeFileSync(path.join(bin, "fzf"), `#!/bin/sh\ntee ${JSON.stringify(path.join(tmp, "shown"))} | head -1\n`, { mode: 0o755 });
execFileSync("tmux", ["-S", socket, "-f", "/dev/null", "new-session", "-d", "-s", "t", "-c", plain, "-x", "120", "-y", "30", "cat"], { env });
execFileSync("tmux", ["-S", socket, "new-window", "-d", "-c", path.join(repo, "src"), "cat"], { env });
after(() => {
  spawnSync("tmux", ["-S", socket, "kill-server"], { env });
  fs.rmSync(tmp, { recursive: true, force: true });
});

test("--repos lists each repository once, as its main clone, the one asked from first, and nothing in no repository", () => {
  const ran = spawnSync("zsh", [path.join(__dirname, "../tmux/pick-dir.zsh"), "--repos", "Enter picks it"], { cwd: worktree, env: { ...env, TMUX: `${socket},1,0` }, encoding: "utf8" });
  assert.equal(ran.status, 0, ran.stderr);
  assert.equal(ran.stdout.trim(), repo, "a worktree asked from gives its main clone");
  assert.deepEqual(fs.readFileSync(path.join(tmp, "shown"), "utf8").trim().split("\n"), ["~/repo"], "a tab inside it is the same repository, and the plain directory and home are left out");
});

test("without --repos every directory is listed, the worktree asked from as it is", () => {
  const ran = spawnSync("zsh", [path.join(__dirname, "../tmux/pick-dir.zsh"), "Enter picks it"], { cwd: worktree, env: { ...env, TMUX: `${socket},1,0` }, encoding: "utf8" });
  assert.equal(ran.status, 0, ran.stderr);
  assert.equal(ran.stdout.trim(), worktree);
  assert.deepEqual(fs.readFileSync(path.join(tmp, "shown"), "utf8").trim().split("\n"), ["~/repo/.claude/worktrees/feature", "~/plain", "~/repo/src", "~"]);
});
