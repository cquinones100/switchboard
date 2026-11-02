// Tests for tmux/repo-update.sh, which offers new commits on GitHub to
// ~/switchboard. Run with: node --test test/*.test.js
//
// A throwaway "remote" (a bare repository) and clones of it, in a temporary
// directory with a temporary HOME and no tmux, so nothing touches your own
// repository, tmux or state.

const { test } = require("node:test");
const assert = require("node:assert/strict");
const { execFileSync, spawnSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const script = path.join(__dirname, "../tmux/repo-update.sh");

test("the update prompt lists the new commits newest first, fast-forwards main on Update now, leaves it on Not now, and refuses when main has commits of its own", () => {
  const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "repo-update-test-")));
  const repo = path.join(tmp, "editor-configs");
  const env = { ...process.env, HOME: tmp, SWITCHBOARD_DIR: repo, TMUX: "", GIT_CONFIG_GLOBAL: "/dev/null", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e" };
  const git = (cwd, ...args) => execFileSync("git", ["-C", cwd, ...args], { env, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
  // What the prompt drew, its colors left out.
  const prompt = (answer) => {
    const ran = spawnSync("bash", [script, "prompt"], { env, input: answer, encoding: "utf8" });
    return { ...ran, stdout: ran.stdout.replace(/\x1b\[[0-9;?]*[A-Za-z]/g, "") };
  };
  try {
    const origin = path.join(tmp, "origin.git");
    const other = path.join(tmp, "other");
    execFileSync("git", ["init", "-q", "--bare", "-b", "main", origin], { env });
    execFileSync("git", ["clone", "-q", origin, repo], { env, stdio: "ignore" });
    git(repo, "commit", "-q", "--allow-empty", "-m", "start");
    git(repo, "push", "-q", "origin", "main");
    execFileSync("git", ["clone", "-q", origin, other], { env, stdio: "ignore" });
    git(other, "commit", "-q", "--allow-empty", "-m", "Show the new thing");
    git(other, "push", "-q", "origin", "main");
    const theirs = git(other, "rev-parse", "HEAD");
    const mine = git(repo, "rev-parse", "HEAD");
    git(repo, "fetch", "-q", "origin");

    const declined = prompt("j\n");
    assert.match(declined.stdout, /1 new commit on GitHub.*newest first:\s+● Show the new thing\s+[0-9a-f]+ · .*ago/s);
    assert.match(declined.stdout, /› Update now\s+Not now/, "Update now chosen to start with");
    assert.doesNotMatch(declined.stdout, /See all/, "nothing more to see");
    assert.equal(git(repo, "rev-parse", "main"), mine, "Not now leaves main where it was");
    const state = path.join(tmp, ".local/state/switchboard");
    assert.equal(fs.readFileSync(path.join(state, "update-offered"), "utf8").trim(), theirs, "and is an answer, so the hourly look doesn't ask again");
    assert.ok(!fs.existsSync(path.join(state, "update-prompt")), "the prompt says it is open only while it is");

    fs.rmSync(path.join(state, "update-offered"));
    prompt("");
    assert.ok(!fs.existsSync(path.join(state, "update-offered")), "a closed input answers nothing");

    const accepted = prompt("\n");
    assert.match(accepted.stdout, /Updated to/, accepted.stdout + accepted.stderr);
    assert.equal(git(repo, "rev-parse", "main"), theirs, "yes fast-forwards main");

    git(other, "commit", "-q", "--allow-empty", "-m", "More");
    git(other, "push", "-q", "origin", "main");
    git(repo, "commit", "-q", "--allow-empty", "-m", "Mine, not pushed");
    git(repo, "fetch", "-q", "origin");
    const diverged = prompt("\n");
    assert.match(diverged.stdout, /1 commit of your own not on GitHub/);
    assert.equal(git(repo, "log", "-1", "--format=%s", "main"), "Mine, not pushed", "and main is left alone");
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});

test("an update prompt left open is replaced by one for newer commits, and only an answer stops the hourly look asking", () => {
  const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "repo-update-open-")));
  const repo = path.join(tmp, "switchboard");
  const bin = path.join(tmp, "bin");
  const calls = path.join(tmp, "tmux-calls");
  fs.mkdirSync(bin);
  // A stand-in tmux: keeps each call, and names one client.
  fs.writeFileSync(path.join(bin, "tmux"), `#!/bin/sh\nprintf '%s\\n' "$*" >> '${calls}'\ncase "$1" in list-clients) echo "1 client-1" ;; esac\n`, { mode: 0o755 });
  const env = { ...process.env, HOME: tmp, SWITCHBOARD_DIR: repo, TMUX: "", PATH: `${bin}:${process.env.PATH}`, GIT_CONFIG_GLOBAL: "/dev/null", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e" };
  const git = (cwd, ...args) => execFileSync("git", ["-C", cwd, ...args], { env, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
  const state = path.join(tmp, ".local/state/switchboard");
  const hourly = () => {
    fs.writeFileSync(calls, "");
    fs.rmSync(path.join(state, "update-checked"), { force: true });
    spawnSync("bash", [script, "periodic"], { env, encoding: "utf8" });
    return fs.readFileSync(calls, "utf8");
  };
  try {
    const origin = path.join(tmp, "origin.git");
    const other = path.join(tmp, "other");
    execFileSync("git", ["init", "-q", "--bare", "-b", "main", origin], { env });
    execFileSync("git", ["clone", "-q", origin, repo], { env, stdio: "ignore" });
    git(repo, "commit", "-q", "--allow-empty", "-m", "start");
    git(repo, "push", "-q", "origin", "main");
    execFileSync("git", ["clone", "-q", origin, other], { env, stdio: "ignore" });
    git(other, "commit", "-q", "--allow-empty", "-m", "First new");
    git(other, "push", "-q", "origin", "main");

    assert.match(hourly(), /display-popup -c client-1 .* prompt client-1/, "offered");
    assert.match(hourly(), /display-popup -c client-1 .* prompt client-1/, "and, unanswered, offered again");

    // A prompt open for that tip on that client (a process named as it runs stands in for it) is left alone.
    fs.mkdirSync(state, { recursive: true });
    const tip = git(repo, "rev-parse", "origin/main");
    // Named as the prompt runs, since a pid reused by something else isn't one.
    const sleeper = require("node:child_process").spawn("bash", ["-c", "sleep 30; :", "repo-update.sh prompt"], { stdio: "ignore", detached: true });
    fs.writeFileSync(path.join(state, "update-prompt"), `${sleeper.pid} ${tip} client-1\n`);
    try {
      assert.doesNotMatch(hourly(), /display-popup/, "one open for the same commits stays");
      git(other, "commit", "-q", "--allow-empty", "-m", "Second new");
      git(other, "push", "-q", "origin", "main");
      const said = hourly();
      assert.match(said, /display-popup -C -c client-1[\s\S]*display-popup -c client-1 .* prompt client-1/, "newer commits close it and open a new one");
    } finally {
      process.kill(-sleeper.pid);
    }

    fs.writeFileSync(path.join(state, "update-offered"), git(repo, "rev-parse", "origin/main"));
    fs.rmSync(path.join(state, "update-prompt"));
    assert.doesNotMatch(hourly(), /display-popup/, "answered, it isn't offered again");
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});

test("Update Switchboard with nothing new still runs the guided setup, saying why, for a clone pulled by hand", () => {
  const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "repo-update-now-")));
  const repo = path.join(tmp, "switchboard");
  const bin = path.join(tmp, "bin");
  const calls = path.join(tmp, "tmux-calls");
  fs.mkdirSync(bin);
  // A stand-in tmux: keeps each call, a line each, and answers nothing.
  fs.writeFileSync(path.join(bin, "tmux"), `#!/bin/sh\nprintf '%s\\n' "$*" >> '${calls}'\n`, { mode: 0o755 });
  const env = { ...process.env, HOME: tmp, SWITCHBOARD_DIR: repo, TMUX: "", PATH: `${bin}:${process.env.PATH}`, GIT_CONFIG_GLOBAL: "/dev/null", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e" };
  try {
    const origin = path.join(tmp, "origin.git");
    execFileSync("git", ["init", "-q", "--bare", "-b", "main", origin], { env });
    execFileSync("git", ["clone", "-q", origin, repo], { env, stdio: "ignore" });
    execFileSync("git", ["-C", repo, "commit", "-q", "--allow-empty", "-m", "start"], { env });
    execFileSync("git", ["-C", repo, "push", "-q", "origin", "main"], { env, stdio: "ignore" });

    spawnSync("bash", [script, "now", "client-1"], { env, encoding: "utf8" });
    const said = fs.readFileSync(calls, "utf8");
    assert.match(said, /display-popup -c client-1 .*-e SAY_TEXT=Looking for Switchboard updates… .*say\.sh box/, "it says it is looking, in a box");
    assert.doesNotMatch(said, /display-message/);
    assert.match(said, /display-popup -c client-1 .*Switchboard setup .*SETUP_WHY=.*up\\? to\\? date.* run-setup/s);

    // The hourly check says nothing and runs nothing when there is nothing new.
    fs.writeFileSync(calls, "");
    fs.rmSync(path.join(tmp, ".local/state/switchboard/update-checked"), { force: true });
    spawnSync("bash", [script, "periodic"], { env, encoding: "utf8" });
    assert.doesNotMatch(fs.readFileSync(calls, "utf8"), /display-popup/);

    // With your CLAUDE.md's rules in a repository, each look pulls it, through
    // setup's CLAUDE.md step (a stand-in setup.sh here, noting how it ran).
    fs.mkdirSync(path.join(tmp, ".config/switchboard"), { recursive: true });
    fs.writeFileSync(path.join(tmp, ".config/switchboard/claude.json"), JSON.stringify({ repo: "https://example.com/rules.git" }));
    fs.writeFileSync(path.join(repo, "setup.sh"), `#!/bin/sh\nprintf '%s\\n' "$*" >> '${path.join(tmp, "setup-calls")}'\n`, { mode: 0o755 });
    fs.rmSync(path.join(tmp, ".local/state/switchboard/update-checked"), { force: true });
    spawnSync("bash", [script, "periodic"], { env, encoding: "utf8" });
    // It runs beside the look, in the background: waited for here.
    for (let i = 0; i < 50 && !fs.existsSync(path.join(tmp, "setup-calls")); i++) execFileSync("sleep", ["0.1"]);
    assert.equal(fs.readFileSync(path.join(tmp, "setup-calls"), "utf8"), "claude_md\n");
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});
