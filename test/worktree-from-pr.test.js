// Tests for bin/worktree-from-pr. Run with: node --test test/*.test.js
//
// Each test builds a throwaway "remote" (a bare repository) and a clone of it
// in a temporary directory, with a stand-in gh on the PATH that answers for
// one pull request, so nothing reaches GitHub or your own repositories.

const { test } = require("node:test");
const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const script = path.join(__dirname, "../bin/worktree-from-pr");

test("a merged PR whose branch the remote deleted opens from the commits GitHub keeps for it", () => {
  const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "wtpr-test-")));
  const env = { ...process.env, HOME: tmp, GIT_CONFIG_GLOBAL: "/dev/null", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e", PATH: `${path.join(tmp, "bin")}:${process.env.PATH}` };
  const git = (cwd, ...args) => execFileSync("git", ["-C", cwd, ...args], { env, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
  try {
    const origin = path.join(tmp, "origin.git");
    const clone = path.join(tmp, "clone");
    execFileSync("git", ["init", "-q", "--bare", "-b", "main", origin], { env });
    execFileSync("git", ["clone", "-q", origin, clone], { env, stdio: "ignore" });
    git(clone, "commit", "-q", "--allow-empty", "-m", "start");
    git(clone, "push", "-q", "origin", "main");
    git(clone, "checkout", "-q", "-b", "feat");
    fs.writeFileSync(path.join(clone, "f.txt"), "feature\n");
    git(clone, "add", "f.txt");
    git(clone, "commit", "-q", "-m", "feature");
    const last = git(clone, "rev-parse", "HEAD");
    git(clone, "push", "-q", "origin", "feat");
    // Merged on GitHub: its commits kept under refs/pull/7/head, its branch
    // deleted, and no trace of it left in this clone.
    git(origin, "update-ref", "refs/pull/7/head", last);
    git(clone, "push", "-q", "origin", "--delete", "feat");
    git(clone, "checkout", "-q", "main");
    git(clone, "branch", "-q", "-D", "feat");
    git(clone, "fetch", "-q", "--prune");

    fs.mkdirSync(path.join(tmp, "bin"));
    fs.writeFileSync(path.join(tmp, "bin/gh"), `#!/bin/sh\n[ "$1" = api ] && { echo t; exit 0; }\necho '${JSON.stringify({ number: 7, title: "Feature", author: { login: "t" }, headRefName: "feat", isCrossRepository: false, state: "MERGED", url: "https://github.com/o/r/pull/7" })}'\n`, { mode: 0o755 });

    const cdFile = path.join(tmp, "cd");
    execFileSync(process.execPath, [script, "--no-claude", "--cd-file", cdFile, "7"], { cwd: clone, env, stdio: ["ignore", "ignore", "pipe"] });
    const worktree = fs.readFileSync(cdFile, "utf8").split("\n")[0];
    assert.equal(path.basename(worktree), "pr-7-feat");
    const prompt = fs.readFileSync(cdFile, "utf8").split("\n")[1];
    assert.match(prompt, /^Pick up PR #7 by t\. It is merged, so say that first\. /, "Claude starts by saying where the PR stands");
    assert.match(prompt, /Don't change anything until I say\.$/, "and waits, on a PR of your own nothing about pushing");
    assert.equal(git(worktree, "rev-parse", "HEAD"), last, "at the PR's last commit");
    assert.equal(git(worktree, "branch", "--show-current"), "feat", "on a local branch of the PR's name");
    assert.throws(() => git(worktree, "rev-parse", "--abbrev-ref", "@{upstream}"), "with no upstream to push to");
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});

test("a PR URL from another repository opens in a clone of that one found among yours, or is refused when there is none", () => {
  const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "wtpr-test-")));
  // TMUX_TMPDIR here, so looking for a clone asks no real tmux server.
  const env = { ...process.env, HOME: tmp, TMUX: "", TMUX_TMPDIR: tmp, GIT_CONFIG_GLOBAL: path.join(tmp, "gitconfig"), GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e", PATH: `${path.join(tmp, "bin")}:${process.env.PATH}` };
  const git = (cwd, ...args) => execFileSync("git", ["-C", cwd, ...args], { env, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
  const run = (cwd, url, cdFile) => {
    try {
      execFileSync(process.execPath, [script, "--no-claude", "--cd-file", cdFile, url], { cwd, env, encoding: "utf8", stdio: ["ignore", "ignore", "pipe"] });
      return "";
    } catch (err) {
      return err.stderr;
    }
  };
  try {
    // Where you are: a clone of acme/widgets, away from your home's top level.
    const here = path.join(tmp, "work", "widgets");
    fs.mkdirSync(here, { recursive: true });
    git(here, "init", "-q");
    git(here, "remote", "add", "origin", "git@github.com:acme/widgets.git");
    // Right under your home: a clone of other/thing. Its GitHub is a bare
    // repository holding the PR's branch, reached through insteadOf, so the
    // clone's remote still reads as other/thing.
    const origin = path.join(tmp, "thing-origin.git");
    const thing = path.join(tmp, "thing");
    execFileSync("git", ["init", "-q", "--bare", "-b", "main", origin], { env });
    fs.writeFileSync(path.join(tmp, "gitconfig"), `[url "${origin}"]\n\tinsteadOf = git@github.com:other/thing.git\n`);
    execFileSync("git", ["clone", "-q", origin, thing], { env, stdio: "ignore" });
    git(thing, "commit", "-q", "--allow-empty", "-m", "start");
    git(thing, "push", "-q", "origin", "main", "main:fix");
    git(thing, "remote", "set-url", "origin", "git@github.com:other/thing.git");
    fs.mkdirSync(path.join(tmp, "bin"));
    fs.writeFileSync(path.join(tmp, "bin/gh"), `#!/bin/sh\necho '${JSON.stringify({ number: 5, title: "Fix", author: { login: "t" }, headRefName: "fix", isCrossRepository: false, state: "OPEN", url: "https://github.com/other/thing/pull/5" })}'\n`, { mode: 0o755 });

    const cdFile = path.join(tmp, "cd");
    assert.equal(run(here, "https://github.com/other/thing/pull/5", cdFile), "");
    assert.equal(fs.readFileSync(cdFile, "utf8").split("\n")[0], path.join(thing, ".claude/worktrees/pr-5-fix"), "made in the clone of other/thing, for wtpr to cd into");
    assert.equal(fs.existsSync(path.join(here, ".claude/worktrees")), false, "not in the one you were in");

    // No clone of it anywhere: refused, nothing made.
    assert.match(run(here, "https://github.com/nobody/nothing/pull/5", path.join(tmp, "cd2")), /That PR is in nobody\/nothing, and no clone of it was found/);
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});

test("a fork's PR opened --trusted opens on <owner>-<branch>, tracking the fork's branch from a remote named after its owner", () => {
  const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "wtpr-test-")));
  const env = { ...process.env, HOME: tmp, GIT_CONFIG_GLOBAL: path.join(tmp, "gitconfig"), GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e", PATH: `${path.join(tmp, "bin")}:${process.env.PATH}` };
  const git = (cwd, ...args) => execFileSync("git", ["-C", cwd, ...args], { env, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
  try {
    // acme/widgets and alice's fork of it, each a bare repository reached
    // through insteadOf, so the remotes still read as GitHub's.
    const origin = path.join(tmp, "widgets.git");
    const forkOrigin = path.join(tmp, "fork.git");
    const clone = path.join(tmp, "clone");
    execFileSync("git", ["init", "-q", "--bare", "-b", "main", origin], { env });
    execFileSync("git", ["init", "-q", "--bare", "-b", "main", forkOrigin], { env });
    fs.writeFileSync(path.join(tmp, "gitconfig"), `[url "${origin}"]\n\tinsteadOf = git@github.com:acme/widgets.git\n[url "${forkOrigin}"]\n\tinsteadOf = git@github.com:alice/widgets.git\n`);
    execFileSync("git", ["clone", "-q", "git@github.com:acme/widgets.git", clone], { env, stdio: "ignore" });
    git(clone, "commit", "-q", "--allow-empty", "-m", "start");
    git(clone, "push", "-q", "origin", "main");
    // The fork's PR comes from its main, which also names a branch here.
    git(clone, "commit", "-q", "--allow-empty", "-m", "alice's fix");
    const last = git(clone, "rev-parse", "HEAD");
    git(clone, "push", "-q", "git@github.com:alice/widgets.git", "HEAD:main");
    git(clone, "reset", "-q", "--hard", "HEAD~1");

    fs.mkdirSync(path.join(tmp, "bin"));
    fs.writeFileSync(path.join(tmp, "bin/gh"), `#!/bin/sh\necho '${JSON.stringify({ number: 9, title: "Fix", author: { login: "alice" }, headRefName: "main", headRepository: { name: "widgets" }, headRepositoryOwner: { login: "alice" }, isCrossRepository: true, maintainerCanModify: true, state: "OPEN", url: "https://github.com/acme/widgets/pull/9" })}'\n`, { mode: 0o755 });

    const cdFile = path.join(tmp, "cd");
    execFileSync(process.execPath, [script, "--no-claude", "--trusted", "--cd-file", cdFile, "9"], { cwd: clone, env, stdio: ["ignore", "ignore", "pipe"] });
    const worktree = fs.readFileSync(cdFile, "utf8").split("\n")[0];
    assert.match(fs.readFileSync(cdFile, "utf8").split("\n")[1], /It's alice's PR, so don't push to it unless I ask\.$/, "someone else's PR isn't pushed to unasked");
    assert.equal(git(clone, "config", "remote.alice.url"), "git@github.com:alice/widgets.git", "the fork added as a remote, over SSH as origin is");
    assert.equal(git(worktree, "rev-parse", "HEAD"), last, "at the fork's commit");
    assert.equal(git(worktree, "branch", "--show-current"), "alice-main", "on a branch that leaves your main alone");
    assert.equal(git(worktree, "rev-parse", "--abbrev-ref", "@{upstream}"), "alice/main", "tracking the fork's branch, so a push updates the PR");
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});

test("a fork's PR opens untrusted by default: outside the clone, its hooks never run, nothing linked, no remote added, and no Claude", () => {
  const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "wtpr-test-")));
  const env = { ...process.env, HOME: tmp, GIT_CONFIG_GLOBAL: "/dev/null", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e", PATH: `${path.join(tmp, "bin")}:${process.env.PATH}` };
  const git = (cwd, ...args) => execFileSync("git", ["-C", cwd, ...args], { env, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
  const ran = path.join(tmp, "ran");
  const hook = `#!/bin/sh\necho "$0" >> ${JSON.stringify(ran)}\n`;
  try {
    const origin = path.join(tmp, "origin.git");
    const clone = path.join(tmp, "clone");
    execFileSync("git", ["init", "-q", "--bare", "-b", "main", origin], { env });
    execFileSync("git", ["clone", "-q", origin, clone], { env, stdio: "ignore" });
    // Your clone's hooks, by their full path, as setup.sh git sets them.
    fs.mkdirSync(path.join(clone, ".githooks"));
    for (const name of ["post-checkout", "post-merge", "post-commit", "pre-commit"]) fs.writeFileSync(path.join(clone, ".githooks", name), hook, { mode: 0o755 });
    git(clone, "add", ".");
    git(clone, "commit", "-q", "-m", "start");
    git(clone, "push", "-q", "origin", "main");
    git(clone, "config", "core.hooksPath", path.join(clone, ".githooks"));
    fs.mkdirSync(path.join(clone, ".claude"));
    fs.writeFileSync(path.join(clone, ".claude/settings.local.json"), "{}");
    // The fork's commit, kept by GitHub under refs/pull/9/head, with hooks of its own.
    git(clone, "checkout", "-q", "-b", "theirs");
    for (const name of ["post-checkout", "post-merge", "post-commit", "pre-commit"]) fs.writeFileSync(path.join(clone, ".githooks", name), hook.replace("$0", "theirs $0"), { mode: 0o755 });
    // And a filter of yours their .gitattributes chooses, which a checkout would run.
    fs.writeFileSync(path.join(clone, ".gitattributes"), "*.txt filter=mine\n");
    fs.writeFileSync(path.join(clone, "f.txt"), "theirs\n");
    git(clone, "add", ".gitattributes", "f.txt");
    git(clone, "commit", "-q", "--no-verify", "-am", "their hooks");
    git(clone, "config", "filter.mine.smudge", `echo filter >> ${JSON.stringify(ran)}; cat`);
    const last = git(clone, "rev-parse", "HEAD");
    git(clone, "push", "-q", "--no-verify", "origin", "theirs");
    git(origin, "update-ref", "refs/pull/9/head", last);
    git(origin, "branch", "-q", "-D", "theirs");
    git(clone, "checkout", "-q", "main");
    git(clone, "branch", "-q", "-D", "theirs");
    fs.rmSync(ran, { force: true });

    fs.mkdirSync(path.join(tmp, "bin"));
    fs.writeFileSync(path.join(tmp, "bin/gh"), `#!/bin/sh\n[ "$1" = api ] && { echo 2015-01-01T00:00:00Z; exit 0; }\ncase "$5" in *baseRefName*) ;; *) echo '{}'; exit 0 ;; esac\necho '${JSON.stringify({ number: 9, title: "Fix", author: { login: "alice" }, headRefName: "main", baseRefName: "main", headRepository: { name: "widgets" }, headRepositoryOwner: { login: "alice" }, isCrossRepository: true, maintainerCanModify: true, state: "OPEN", changedFiles: 4, additions: 4, deletions: 4, url: "https://github.com/acme/widgets/pull/9" })}'\n`, { mode: 0o755 });

    const cdFile = path.join(tmp, "cd");
    let status = 0;
    let stderr = "";
    try {
      execFileSync(process.execPath, [script, "--no-claude", "--cd-file", cdFile, "9"], { cwd: clone, env, encoding: "utf8", stdio: ["ignore", "ignore", "pipe"] });
    } catch (err) {
      status = err.status;
      stderr = err.stderr;
    }
    assert.equal(status, 3, "which the wtpr function reads as nothing to go into");
    assert.ok(!fs.existsSync(cdFile) || !fs.readFileSync(cdFile, "utf8"), "and no directory or prompt handed to it");
    const worktree = path.join(tmp, ".cache/switchboard/pr-review", path.basename(clone), "9");
    assert.equal(git(worktree, "rev-parse", "HEAD"), last, "at the PR's commit");
    assert.equal(git(worktree, "branch", "--show-current"), "pr-review/9");
    assert.equal(git(worktree, "config", "core.hooksPath"), "/dev/null", "hooks off for that worktree");
    assert.equal(git(clone, "config", "core.hooksPath"), path.join(clone, ".githooks"), "and still on in your clone");
    assert.equal(git(worktree, "config", "switchboard.untrusted"), "true");
    assert.equal(git(worktree, "config", "core.fsmonitor"), "false", "and no file-system monitor command");
    assert.ok(!fs.existsSync(path.join(worktree, ".claude/settings.local.json")), "nothing linked");
    assert.equal(git(clone, "remote"), "origin", "no remote added for the fork");
    assert.match(stderr, /Author: {3}alice \(on GitHub \d+ years\)/);
    assert.match(stderr, /\.githooks\/post-checkout/, "pr-risk lists the hooks it changes");

    fs.writeFileSync(path.join(worktree, "g.txt"), "x\n");
    git(worktree, "add", "g.txt");
    git(worktree, "commit", "-q", "-m", "a commit there");
    execFileSync("git", ["-C", worktree, "merge", "-q", "--no-edit", "--no-ff", "main"], { env, stdio: "ignore" });
    assert.ok(!fs.existsSync(ran), "no hook or filter ran, theirs or yours, on checkout, commit or merge");

    // Run again from another worktree of the same clone, it reuses the review worktree.
    git(worktree, "reset", "-q", "--hard", last);
    git(clone, "worktree", "add", "-q", "-b", "other", path.join(tmp, "other"));
    let again = 0;
    try {
      execFileSync(process.execPath, [script, "--no-claude", "--cd-file", cdFile, "9"], { cwd: path.join(tmp, "other"), env, encoding: "utf8", stdio: ["ignore", "ignore", "pipe"] });
    } catch (err) {
      again = err.status;
      assert.match(err.stderr, /moved to the PR's newest commit/, err.stderr);
    }
    assert.equal(again, 3);

    // Claude is never started there, and nothing lists it as work to reopen.
    const here = require("node:child_process").spawnSync(process.execPath, [path.join(__dirname, "../bin/claude-here"), "--print"], { cwd: worktree, env, encoding: "utf8" });
    assert.equal(here.status, 1);
    assert.match(here.stderr, /Claude isn't started there/);
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});
