const test = require("node:test");
const assert = require("node:assert");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { execFileSync, spawnSync } = require("node:child_process");

const repo = path.join(__dirname, "..");

test("pre-push refuses a push to main whose tip /review-loop hasn't found clean, and lets other branches through", () => {
  const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "pre-push-")));
  // Its own settings file, which isn't there, so none in the environment counts.
  const env = { ...process.env, HOME: tmp, SWITCHBOARD_SETTINGS: path.join(tmp, "no-settings.json"), GIT_CONFIG_GLOBAL: "/dev/null", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e" };
  try {
    execFileSync("git", ["init", "-q", "-b", "main", tmp], { env });
    fs.writeFileSync(path.join(tmp, "a.txt"), "a\n");
    execFileSync("git", ["-C", tmp, "add", "."], { env });
    execFileSync("git", ["-C", tmp, "commit", "-q", "-m", "first"], { env });
    const base = execFileSync("git", ["-C", tmp, "rev-parse", "HEAD"], { env, encoding: "utf8" }).trim();
    execFileSync("git", ["-C", tmp, "commit", "-q", "--allow-empty", "-m", "second"], { env });
    const middle = execFileSync("git", ["-C", tmp, "rev-parse", "HEAD"], { env, encoding: "utf8" }).trim();
    execFileSync("git", ["-C", tmp, "commit", "-q", "--allow-empty", "-m", "third"], { env });
    const sha = execFileSync("git", ["-C", tmp, "rev-parse", "HEAD"], { env, encoding: "utf8" }).trim();
    // Main on the remote is at the first commit; the push adds the other two.
    const push = (ref) => spawnSync(path.join(repo, ".githooks/pre-push"), ["origin", "url"], { cwd: tmp, env, encoding: "utf8", input: `refs/heads/main ${sha} ${ref} ${base}\n` });

    const unreviewed = push("refs/heads/main");
    assert.equal(unreviewed.status, 1);
    assert.match(unreviewed.stderr, /hasn't been through \/review-loop/);
    assert.equal(push("refs/heads/feature").status, 0, "another branch isn't asked about");

    // Reviewed from the commit main is at on the remote: every commit it adds.
    // Turned off in Settings, it asks for nothing.
    fs.writeFileSync(path.join(tmp, "settings.json"), JSON.stringify({ review_before_push: "false" }));
    assert.equal(spawnSync(path.join(repo, ".githooks/pre-push"), ["origin", "url"], { cwd: tmp, env: { ...env, SWITCHBOARD_SETTINGS: path.join(tmp, "settings.json") }, encoding: "utf8", input: `refs/heads/main ${sha} refs/heads/main ${base}\n` }).status, 1, "a string false is not off");
    fs.writeFileSync(path.join(tmp, "settings.json"), "{}\n" + JSON.stringify({ review_before_push: false }));
    assert.equal(spawnSync(path.join(repo, ".githooks/pre-push"), ["origin", "url"], { cwd: tmp, env: { ...env, SWITCHBOARD_SETTINGS: path.join(tmp, "settings.json") }, encoding: "utf8", input: `refs/heads/main ${sha} refs/heads/main ${base}\n` }).status, 1, "nor a file of two documents");
    fs.writeFileSync(path.join(tmp, "settings.json"), "{\"review_before_push\":false,\"other\":NaN}");
    assert.equal(spawnSync(path.join(repo, ".githooks/pre-push"), ["origin", "url"], { cwd: tmp, env: { ...env, SWITCHBOARD_SETTINGS: path.join(tmp, "settings.json") }, encoding: "utf8", input: `refs/heads/main ${sha} refs/heads/main ${base}\n` }).status, 1, "nor one only a looser parser takes");
    fs.writeFileSync(path.join(tmp, "settings.json"), JSON.stringify({ review_before_push: false }));
    assert.equal(spawnSync(path.join(repo, ".githooks/pre-push"), ["origin", "url"], { cwd: tmp, env: { ...env, SWITCHBOARD_SETTINGS: path.join(tmp, "settings.json") }, encoding: "utf8", input: `refs/heads/main ${sha} refs/heads/main ${base}\n` }).status, 0);

    fs.writeFileSync(path.join(tmp, ".git/review-loop-clean"), `${sha} ${base}\n`);
    assert.equal(push("refs/heads/main").status, 0, "the commit the review found clean");
    // Reviewed from a later commit than main is at: the ones between weren't.
    fs.writeFileSync(path.join(tmp, ".git/review-loop-clean"), `${sha} ${middle}\n`);
    const partly = push("refs/heads/main");
    assert.equal(partly.status, 1);
    assert.match(partly.stderr, /didn't cover every commit/);
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});

test("pre-push takes the reviewed commit rebased unchanged onto a newer main, but not one rebased past a change beside its own, nor one with more on top", () => {
  const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "pre-push-patch-")));
  const env = { ...process.env, HOME: tmp, SWITCHBOARD_SETTINGS: path.join(tmp, "no-settings.json"), GIT_CONFIG_GLOBAL: "/dev/null", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e" };
  const lines = Array.from({ length: 30 }, (_, i) => `line ${i + 1}`);
  try {
    execFileSync("git", ["init", "-q", "-b", "main", tmp], { env });
    fs.writeFileSync(path.join(tmp, "a.txt"), lines.join("\n") + "\n");
    execFileSync("git", ["-C", tmp, "add", "."], { env });
    execFileSync("git", ["-C", tmp, "commit", "-q", "-m", "base"], { env });
    const base = execFileSync("git", ["-C", tmp, "rev-parse", "HEAD"], { env, encoding: "utf8" }).trim();
    // The work, reviewed on the base: line 20 changed.
    fs.writeFileSync(path.join(tmp, "a.txt"), lines.map((l, i) => (i === 19 ? "work" : l)).join("\n") + "\n");
    execFileSync("git", ["-C", tmp, "commit", "-q", "-am", "work"], { env });
    const work = execFileSync("git", ["-C", tmp, "rev-parse", "HEAD"], { env, encoding: "utf8" }).trim();
    const patch = execFileSync(path.join(repo, "bin/codex-review"), ["--patch-id", base, work], { cwd: tmp, env, encoding: "utf8" }).trim();
    assert.match(patch, /^[0-9a-f]{40}$/);
    fs.writeFileSync(path.join(tmp, ".git/review-loop-clean"), `${work} ${base} ${patch}\n`);

    // Main gained a change far from it (line 2), and the work was rebased onto it.
    execFileSync("git", ["-C", tmp, "checkout", "-q", "-b", "far", base], { env });
    fs.writeFileSync(path.join(tmp, "a.txt"), lines.map((l, i) => (i === 1 ? "far" : l)).join("\n") + "\n");
    execFileSync("git", ["-C", tmp, "commit", "-q", "-am", "far"], { env });
    const far = execFileSync("git", ["-C", tmp, "rev-parse", "HEAD"], { env, encoding: "utf8" }).trim();
    execFileSync("git", ["-C", tmp, "cherry-pick", work], { env, stdio: "ignore" });
    const rebased = execFileSync("git", ["-C", tmp, "rev-parse", "HEAD"], { env, encoding: "utf8" }).trim();
    const push = (tip, remote) => spawnSync(path.join(repo, ".githooks/pre-push"), ["origin", "url"], { cwd: tmp, env, encoding: "utf8", input: `refs/heads/main ${tip} refs/heads/main ${remote}\n` });
    assert.equal(push(rebased, far).status, 0, "the same change, on the newer main");
    // The same change under a message the review didn't read.
    execFileSync("git", ["-C", tmp, "commit", "-q", "--amend", "-m", "reworded"], { env });
    const reworded = execFileSync("git", ["-C", tmp, "rev-parse", "HEAD"], { env, encoding: "utf8" }).trim();
    assert.equal(push(reworded, far).status, 1);
    execFileSync("git", ["-C", tmp, "reset", "-q", "--hard", rebased], { env });

    // More on top of it, even when it leaves the same change (something added
    // and taken out again): only one commit is taken this way.
    fs.writeFileSync(path.join(tmp, "b.txt"), "b\n");
    execFileSync("git", ["-C", tmp, "add", "b.txt"], { env });
    execFileSync("git", ["-C", tmp, "commit", "-q", "-m", "more"], { env });
    const more = execFileSync("git", ["-C", tmp, "rev-parse", "HEAD"], { env, encoding: "utf8" }).trim();
    const added = push(more, far);
    assert.equal(added.status, 1);
    assert.match(added.stderr, /hasn't been through \/review-loop/);
    execFileSync("git", ["-C", tmp, "rm", "-q", "b.txt"], { env });
    execFileSync("git", ["-C", tmp, "commit", "-q", "-m", "taken out"], { env });
    const takenOut = execFileSync("git", ["-C", tmp, "rev-parse", "HEAD"], { env, encoding: "utf8" }).trim();
    assert.equal(push(takenOut, far).status, 1);

    // Main gained a change beside it (line 18, inside its context): the rebased
    // change reads differently, so it needs a review of its own.
    execFileSync("git", ["-C", tmp, "checkout", "-q", "-b", "near", base], { env });
    fs.writeFileSync(path.join(tmp, "a.txt"), lines.map((l, i) => (i === 17 ? "near" : l)).join("\n") + "\n");
    execFileSync("git", ["-C", tmp, "commit", "-q", "-am", "near"], { env });
    const near = execFileSync("git", ["-C", tmp, "rev-parse", "HEAD"], { env, encoding: "utf8" }).trim();
    execFileSync("git", ["-C", tmp, "cherry-pick", work], { env, stdio: "ignore" });
    const beside = execFileSync("git", ["-C", tmp, "rev-parse", "HEAD"], { env, encoding: "utf8" }).trim();
    assert.equal(push(beside, near).status, 1);
    // Nor with git set to show no context, which would leave the change beside it out.
    assert.equal(spawnSync(path.join(repo, ".githooks/pre-push"), ["origin", "url"], { cwd: tmp, env: { ...env, GIT_CONFIG_COUNT: "1", GIT_CONFIG_KEY_0: "diff.context", GIT_CONFIG_VALUE_0: "0" }, encoding: "utf8", input: `refs/heads/main ${beside} refs/heads/main ${near}\n` }).status, 1);

    // Reviewed on a base main on the remote doesn't have yet: the change may
    // need what that base added.
    fs.writeFileSync(path.join(tmp, ".git/review-loop-clean"), `${work} ${far} ${patch}\n`);
    execFileSync("git", ["-C", tmp, "checkout", "-q", "-b", "older", base], { env });
    execFileSync("git", ["-C", tmp, "cherry-pick", work], { env, stdio: "ignore" });
    const older = execFileSync("git", ["-C", tmp, "rev-parse", "HEAD"], { env, encoding: "utf8" }).trim();
    assert.equal(push(older, base).status, 1);

    // A record from before the patch was kept: the exact commit only.
    fs.writeFileSync(path.join(tmp, ".git/review-loop-clean"), `${work} ${base}\n`);
    assert.equal(push(rebased, far).status, 1);
    assert.equal(push(work, base).status, 0);
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});

test("post-checkout copies the main clone's lib packages into a new worktree with the same lockfile, and runs no npm", () => {
  const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "post-checkout-")));
  const bin = path.join(tmp, "bin");
  const log = path.join(tmp, "npm.log");
  fs.mkdirSync(bin);
  // A stand-in npm, which must never run: the worktree may be a fork's, whose .npmrc can run its code.
  fs.writeFileSync(path.join(bin, "npm"), `#!/bin/sh\necho "$PWD $*" >> ${JSON.stringify(log)}\n`, { mode: 0o755 });
  const env = { ...process.env, HOME: tmp, PATH: `${bin}:${process.env.PATH}`, GIT_CONFIG_GLOBAL: "/dev/null", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e" };
  const clone = path.join(tmp, "clone");
  const git = (...args) => spawnSync("git", ["-C", clone, ...args], { env, encoding: "utf8" });
  const has = (dir) => fs.existsSync(path.join(tmp, dir, "lib/node_modules/ical.js/package.json"));
  try {
    execFileSync("git", ["init", "-q", "-b", "main", clone], { env });
    fs.mkdirSync(path.join(clone, "lib/node_modules/ical.js"), { recursive: true });
    fs.writeFileSync(path.join(clone, "lib/node_modules/ical.js/package.json"), "{}\n");
    fs.writeFileSync(path.join(clone, "lib/package-lock.json"), "{}\n");
    fs.writeFileSync(path.join(clone, ".gitignore"), "node_modules/\n");
    // The hook committed, and core.hooksPath relative, as setup.sh git set it before it was made absolute, so git runs the new worktree's copy.
    fs.mkdirSync(path.join(clone, ".githooks"));
    fs.copyFileSync(path.join(repo, ".githooks/post-checkout"), path.join(clone, ".githooks/post-checkout"));
    git("add", ".");
    git("commit", "-q", "-m", "first");
    git("config", "core.hooksPath", ".githooks");

    git("worktree", "add", "-q", "-b", "work", path.join(tmp, "work"));
    assert.ok(has("work"), "the main clone's packages, copied");
    assert.ok(!fs.existsSync(path.join(tmp, "work/lib/node_modules/node_modules")));

    // Another lockfile, as a fork's PR may bring: nothing is installed, and it says so.
    git("checkout", "-q", "-b", "fork");
    fs.writeFileSync(path.join(clone, "lib/package-lock.json"), '{"changed":true}\n');
    git("commit", "-q", "-am", "another lockfile");
    git("checkout", "-q", "main");
    const fork = git("worktree", "add", "-q", path.join(tmp, "fork"), "fork");
    assert.ok(!has("fork"));
    assert.match(fork.stderr, /run npm ci in lib once you trust its lockfile/);

    fs.rmSync(path.join(tmp, "work/lib/node_modules"), { recursive: true });
    execFileSync("git", ["-C", path.join(tmp, "work"), "checkout", "-q", "-b", "again"], { env, stdio: "ignore" });
    assert.ok(!has("work"), "a branch switched to in a worktree copies nothing");
    assert.ok(!fs.existsSync(log), "npm never ran");
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});

test("this repository's Claude settings run the push guard and the hook guard before every command", () => {
  const settings = JSON.parse(fs.readFileSync(path.join(repo, ".claude/settings.json"), "utf8"));
  const commands = settings.hooks.PreToolUse.filter((h) => h.matcher === "Bash").flatMap((h) => h.hooks.map((x) => x.command));
  assert.ok(commands.some((c) => c.includes("$CLAUDE_PROJECT_DIR/claude/hooks/enforce-push-review.sh")));
  assert.ok(commands.some((c) => c.includes("$CLAUDE_PROJECT_DIR/claude/hooks/enforce-git-hooks-run.sh")), "which keeps core.hooksPath from switching the hook off");
});

test("this repository's push guard blocks git push --no-verify, however it is spelled, but not a dry run or a commit", () => {
  const guard = (command) => spawnSync("bash", [path.join(repo, "claude/hooks/enforce-push-review.sh")], { input: JSON.stringify({ tool_input: { command } }), encoding: "utf8" });
  const commitGuard = (command) => spawnSync("bash", [path.join(repo, "claude/hooks/enforce-git-hooks-run.sh")], { input: JSON.stringify({ tool_input: { command } }), encoding: "utf8" });
  assert.equal(guard("git push --no-verify origin main").status, 2);
  assert.equal(guard("git -C ~/switchboard push -q --no-verify").status, 2);
  assert.equal(guard('git push origin main "--no-verify"').status, 2, "quoted, the shell still passes the flag");
  assert.equal(guard("git push origin main --no\\-verify").status, 2, "escaped, the shell still passes the flag");
  assert.equal(guard("git push origin main --no-verify; echo done").status, 2, "followed by another command");
  assert.equal(guard("git push origin main --no-verify>/dev/null").status, 2, "followed by a redirection");
  assert.equal(guard('git -C "/a/review repo" push origin main --no-verify').status, 2, "in a repository whose path has a space");
  assert.equal(guard("/usr/bin/git push --no-verify origin main").status, 2, "git by its path");
  assert.equal(guard("git push --no-veri origin main").status, 2, "an abbreviation git takes");
  assert.equal(commitGuard("git commit --no-veri -m x").status, 2, "for a commit too");
  assert.equal(commitGuard("git commit --no-verbose -m x").status, 0, "--no-verbose skips nothing");
  assert.equal(guard("git push --no-verbose origin main").status, 0);
  assert.equal(commitGuard('git commit -m "skip with --no-verify, never"').status, 0, "a commit message that mentions it");
  assert.equal(commitGuard("git commit -nm 'x'").status, 2);
  assert.equal(guard("git -c 'core.hooksPath=/dev/null' push origin main").status, 2, "nor switch the hook off for the push");
  assert.equal(guard("GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.HooksPath GIT_CONFIG_VALUE_0=/x git push origin main").status, 2);
  assert.equal(guard("git push -n origin main").status, 0);
  assert.equal(guard("git push -q origin main").status, 0);
  assert.equal(commitGuard("git push --no-verify origin main").status, 0, "the guard every repository gets leaves pushes alone");
  assert.equal(commitGuard("git config core.hookspath /dev/null").status, 2, "git's keys are case-insensitive");
  assert.equal(commitGuard("git config --get core.hooksPath").status, 0, "reading it is fine");
  assert.equal(commitGuard("/usr/bin/git config core.hooksPath /dev/null").status, 2, "git by its path");
  assert.equal(commitGuard("/usr/bin/git commit -n -m x").status, 2);
});

test("with core.hooksPath absolute, as setup.sh git sets it, a new worktree runs the clone's hooks, never its branch's", () => {
  const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "hooks-abs-")));
  const env = { ...process.env, HOME: tmp, GIT_CONFIG_GLOBAL: "/dev/null", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e" };
  const clone = path.join(tmp, "clone");
  const git = (...args) => execFileSync("git", ["-C", clone, ...args], { env, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
  const hook = (marker) => `#!/bin/sh\ntouch ${JSON.stringify(path.join(tmp, marker))}\n`;
  try {
    execFileSync("git", ["init", "-q", "-b", "main", clone], { env });
    fs.mkdirSync(path.join(clone, ".githooks"));
    fs.writeFileSync(path.join(clone, ".githooks/post-checkout"), hook("ours"), { mode: 0o755 });
    git("add", ".");
    git("commit", "-q", "-m", "first");
    // A branch, as a fork's PR may be, whose own hook would run anything.
    git("checkout", "-q", "-b", "theirs");
    fs.writeFileSync(path.join(clone, ".githooks/post-checkout"), hook("theirs"), { mode: 0o755 });
    git("commit", "-q", "-am", "their hook");
    git("checkout", "-q", "main");
    fs.rmSync(path.join(tmp, "ours"), { force: true });
    git("config", "core.hooksPath", path.join(clone, ".githooks"));

    git("worktree", "add", "-q", path.join(tmp, "work"), "theirs");
    assert.ok(fs.existsSync(path.join(tmp, "ours")), "the clone's hook ran");
    assert.ok(!fs.existsSync(path.join(tmp, "theirs")), "the branch's never did");
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});
