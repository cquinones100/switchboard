// Tests for tmux/diff-base.sh, which M-d and the diff story told ahead of
// time share to pick what a branch is compared with. Run with:
// node --test test/*.test.js
//
// A throwaway origin and clone, with a stand-in gh, so nothing reaches
// GitHub or your repositories.

const { test, after } = require("node:test");
const assert = require("node:assert/strict");
const { execFileSync, spawnSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "diff-base-")));
after(() => fs.rmSync(tmp, { recursive: true, force: true }));
const script = path.join(__dirname, "../tmux/diff-base.sh");

test("diff-base.sh fetches the PR's base, keeps it for the branch's commit, and asks again once the branch moves", () => {
  const bin = path.join(tmp, "bin");
  fs.mkdirSync(bin);
  fs.writeFileSync(path.join(bin, "gh"), "#!/bin/sh\necho develop\n", { mode: 0o755 });
  const env = { ...process.env, HOME: tmp, XDG_CACHE_HOME: path.join(tmp, "cache"), XDG_STATE_HOME: path.join(tmp, "state"), PATH: `${bin}:${process.env.PATH}`, GIT_CONFIG_GLOBAL: "/dev/null", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e", TMUX: "" };
  const git = (cwd, ...args) => execFileSync("git", ["-C", cwd, ...args], { env, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
  const origin = path.join(tmp, "origin.git");
  const clone = path.join(tmp, "clone");
  execFileSync("git", ["init", "-q", "--bare", "-b", "develop", origin], { env });
  execFileSync("git", ["clone", "-q", origin, clone], { env, stdio: "ignore" });
  git(clone, "commit", "-q", "--allow-empty", "-m", "start");
  git(clone, "push", "-q", "origin", "HEAD:develop");
  git(clone, "checkout", "-q", "-b", "feature");
  git(clone, "commit", "-q", "--allow-empty", "-m", "work");
  const run = (mode) => spawnSync("bash", [script, mode, clone], { env, encoding: "utf8" });

  assert.equal(run("cached").status, 4, "nothing kept yet");
  const fetched = run("fetch");
  assert.equal(fetched.status, 0);
  assert.equal(fetched.stdout.trim(), "origin/develop", "the PR's base, as origin has it");
  assert.equal(run("cached").stdout.trim(), "origin/develop", "kept for the branch's commit");
  git(clone, "commit", "-q", "--allow-empty", "-m", "more");
  assert.equal(run("cached").status, 4, "asked again once the branch moves");
  assert.equal(spawnSync("bash", [script, "cached", tmp], { env }).status, 1, "nothing outside a repository");
});
