// Tests for bin/switchboard-try --sealed. Run with: node --test test/*.test.js
//
// A temporary HOME holding a stand-in Switchboard clone and a worktree of it
// marked as wtpr --untrusted marks one; the try's home is /tmp/sb-<hash> as
// always, removed with --clean. Its setup.sh is a stand-in, and --run probes
// the sandbox, so no tmux server starts.

const { test } = require("node:test");
const assert = require("node:assert/strict");
const { execFileSync, spawn, spawnSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const script = path.join(__dirname, "../bin/switchboard-try");

test("switchboard-try --sealed runs an untrusted worktree's code in a sandbox that can't read your home or reach your sockets, with every service off, and only sealed", { skip: process.platform !== "darwin" && "needs macOS's sandbox-exec" }, async () => {
  const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "try-sealed-")));
  const env = { ...process.env, HOME: tmp, GIT_CONFIG_GLOBAL: "/dev/null", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e" };
  delete env.TMUX;
  const clone = path.join(tmp, "switchboard");
  const worktree = path.join(tmp, ".cache/switchboard/pr-review/1");
  const other = path.join(tmp, "other");
  const git = (...args) => execFileSync("git", args, { env, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
  const run = (...args) => spawnSync(script, args, { env, encoding: "utf8" });
  let server;
  const other_try = "/tmp/sb-0000beef";
  try {
    fs.mkdirSync(path.join(clone, "tmux"), { recursive: true });
    fs.mkdirSync(path.join(clone, "lib/sidebar"), { recursive: true });
    fs.writeFileSync(path.join(clone, "setup.sh"), "#!/bin/sh\nexit 0\n", { mode: 0o755 });
    fs.writeFileSync(path.join(clone, "tmux/tmux.conf"), "");
    fs.writeFileSync(path.join(clone, "lib/sidebar/model.js"), "");
    git("init", "-q", "-b", "main", clone);
    git("-C", clone, "add", ".");
    git("-C", clone, "commit", "-q", "-m", "start");
    git("-C", clone, "worktree", "add", "-q", "-b", "pr-review/1", worktree);
    git("-C", clone, "config", "extensions.worktreeConfig", "true");
    git("-C", worktree, "config", "--worktree", "switchboard.untrusted", "true");
    fs.writeFileSync(path.join(tmp, "secret.txt"), "a secret\n");
    // Another try's home, which may link your files.
    fs.mkdirSync(other_try, { recursive: true });
    fs.writeFileSync(path.join(other_try, "f"), "another try\n");
    // A socket of yours, as Notes' Neovim listens on one, taking commands from whoever connects.
    const sock = path.join(tmp, "s.sock");
    server = spawn(process.execPath, ["-e", `require("net").createServer((c) => c.end("hi")).listen(${JSON.stringify(sock)})`]);
    while (!fs.existsSync(sock)) await new Promise((r) => setTimeout(r, 50));
    fs.cpSync(clone, other, { recursive: true });

    const open = run(worktree, "--print");
    assert.equal(open.status, 1);
    assert.match(open.stderr, /whose code runs only sealed: switchboard-try --sealed/, "never tried with your files linked in");

    const probe = `cat ${tmp}/secret.txt; echo "read=$?"; cat ${other_try}/f; echo "other=$?"; touch ${worktree}/x; echo "wrote=$?"; echo "token=\${GH_TOKEN:-none}"; node -e 'require("net").connect(process.argv[1]).on("data", () => { console.log("sock=open"); process.exit(); }).on("error", (e) => console.log("sock=" + e.code))' ${tmp}/s.sock; cat ~/.config/switchboard/settings.json`;
    const sealed = spawnSync(script, ["--sealed", worktree, "--run", "/bin/sh", "-c", probe], { env: { ...env, GH_TOKEN: "t0ken" }, encoding: "utf8" });
    assert.match(sealed.stdout, /read=1/, sealed.stdout + sealed.stderr);
    assert.doesNotMatch(sealed.stdout, /a secret/, "your home unreadable");
    assert.match(sealed.stdout, /wrote=1/, "the worktree read-only");
    assert.match(sealed.stdout, /other=1/, "another try's home unreadable");
    assert.match(sealed.stdout, /token=none/, "an environment made from nothing");
    assert.match(sealed.stdout, /sock=EPERM/, "a socket of yours can't be connected to");
    const settings = JSON.parse(sealed.stdout.slice(sealed.stdout.indexOf("{")));
    for (const key of ["github", "linear", "slack_background", "diff_story", "meeting_notes", "standup_note"]) assert.equal(settings[key], false, `${key} off`);
    assert.equal(settings.update_check, "off");

    const elsewhere = run("--sealed", other, "--run", "true");
    assert.equal(elsewhere.status, 1);
    assert.match(elsewhere.stderr, /isn't a worktree of your ~\/switchboard clone/, "a directory neither yours nor untrusted isn't run");
  } finally {
    server?.kill();
    fs.rmSync(other_try, { recursive: true, force: true });
    run("--clean", worktree);
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});
