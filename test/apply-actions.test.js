// Tests for bin/apply-actions, which lists what a landed change needs to take
// effect, and tmux/apply.sh, which does Switchboard's part of it. Run with:
// node --test test/*.test.js
//
// Each test uses throwaway repositories and a temporary HOME under /tmp, with
// no tmux but a throwaway server of its own, so nothing touches your
// repository, tmux or state.

const { test } = require("node:test");
const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const script = path.join(__dirname, "../bin/apply-actions");
const applySh = path.join(__dirname, "../tmux/apply.sh");

const setup = () => {
  const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "apply-actions-")));
  const home = path.join(tmp, "home");
  fs.mkdirSync(home);
  const env = { ...process.env, HOME: home, TMUX: "", GIT_CONFIG_GLOBAL: "/dev/null", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e" };
  const repo = path.join(tmp, "repo");
  const git = (...args) => execFileSync("git", ["-C", repo, ...args], { env, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
  execFileSync("git", ["init", "-q", "-b", "main", repo], { env });
  // Switchboard is ~/switchboard, by its real path.
  const asSwitchboard = () => fs.symlinkSync(repo, path.join(home, "switchboard"));
  const write = (file, text) => {
    fs.mkdirSync(path.dirname(path.join(repo, file)), { recursive: true });
    fs.writeFileSync(path.join(repo, file), text, { mode: file.endsWith(".sh") ? 0o755 : 0o644 });
  };
  const commit = (files) => {
    for (const [file, text] of Object.entries(files)) write(file, text);
    git("add", "-A");
    git("commit", "-q", "-m", "change");
    return git("rev-parse", "HEAD");
  };
  // The actions for the last commit, as { key: [label, why, command] }.
  const actions = (from) =>
    Object.fromEntries(
      execFileSync(script, [repo, from, "HEAD"], { env, encoding: "utf8" })
        .split("\n")
        .filter(Boolean)
        .map((line) => {
          const [key, ...rest] = line.split("\t");
          return [key, rest];
        }),
    );
  return { tmp, home, env, repo, git, write, commit, actions, asSwitchboard };
};

test("in Switchboard, a change to the sidebar and tmux.conf lists a daemon restart and a reload once each, and docs alone list nothing", () => {
  const { tmp, home, commit, actions, asSwitchboard, git } = setup();
  try {
    asSwitchboard();
    // Switchboard's shape; a setup.sh with nothing left to do.
    const start = commit({ "setup.sh": "#!/bin/sh\necho Ready.\n", "bin/tmux-sidebar": "", "README.md": "a" });
    commit({ "lib/sidebar/model.js": "1", "lib/sidebar/render.js": "2", "tmux/tmux.conf": "set -g x 1\n" });
    const listed = actions(start);
    assert.deepEqual(Object.keys(listed), ["sidebar", "reload"]);
    assert.equal(listed.sidebar[0], "Restart the sidebar daemon");
    assert.equal(listed.sidebar[1], "lib/sidebar/model.js and 1 more changed");
    assert.equal(listed.sidebar[2], "~/.config/tmux/apply.sh sidebar");
    assert.equal(listed.reload[2], "~/.config/tmux/apply.sh reload");

    const loaded = commit({ "tmux/tmux.conf": "run-shell -b '~/.config/tmux/sidebar.sh start'\nbind x run-shell '~/.config/tmux/key.sh'\n" });
    commit({ "tmux/sidebar.sh": "2", "tmux/key.sh": "2" });
    assert.deepEqual(Object.keys(actions(loaded)), ["reload"], "a script tmux.conf runs as it loads, not one a key runs");

    const plugins = git("rev-parse", "HEAD");
    commit({ "nvim/lazy-lock.json": "{}" });
    assert.deepEqual(Object.keys(actions(plugins)), ["lazy"], "Neovim's plugins at the lockfile's commits");

    const deps = git("rev-parse", "HEAD");
    commit({ "lib/package-lock.json": "{}", "lib/sidebar/model.js": "3" });
    assert.deepEqual(Object.keys(actions(deps)), ["install:lib", "sidebar"], "lib's packages installed before the daemon restarts, hooks or not");

    // A daemon started after the change was on disk runs its code already.
    const later = git("rev-parse", "HEAD");
    commit({ "lib/sidebar/model.js": "4" });
    const pidFile = path.join(home, ".local/state/tmux-sidebar/daemon.pid");
    fs.mkdirSync(path.dirname(pidFile), { recursive: true });
    fs.writeFileSync(pidFile, `${process.pid} x\n`);
    const future = new Date(Date.now() + 60000);
    fs.utimesSync(pidFile, future, future);
    assert.deepEqual(actions(later), {}, "setup's tmux part restarted it");
    const gone = git("rev-parse", "HEAD");
    git("rm", "-q", "lib/sidebar/render.js");
    git("commit", "-q", "-m", "drop");
    assert.deepEqual(Object.keys(actions(gone)), ["sidebar"], "a deleted file it may still have loaded asks again");
    fs.rmSync(pidFile);

    const before = commit({ "README.md": "b" });
    commit({ "README.md": "c", "bin/README.md": "d" });
    assert.deepEqual(actions(before), {}, "docs need nothing");
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});

test("in Switchboard, a part setup.sh --check has left to do is listed only when the change touched its files", () => {
  const { tmp, commit, actions, asSwitchboard, git } = setup();
  try {
    asSwitchboard();
    const left = "#!/bin/sh\necho 'Left to do:'\necho '  ✗ Ghostty has links to make: ./setup.sh ghostty'\nexit 1\n";
    const start = commit({ "setup.sh": left, "bin/tmux-sidebar": "" });
    commit({ "ghostty/config": "x" });
    const listed = actions(start);
    assert.deepEqual(Object.keys(listed), ["setup-ghostty", "ghostty"]);
    assert.equal(listed.ghostty[2], "", "reloading Ghostty's config is yours to do");
    assert.match(listed["setup-ghostty"][2], /setup\.sh' ghostty$/);

    const several = commit({ "setup.sh": "#!/bin/sh\necho 'Left to do:'\necho '  ✗ the workspace has links to make: ./setup.sh tmux neovim bin'\necho \"Run ./setup.sh again once they're done.\"\nexit 1\n" });
    commit({ "bin/new-tool": "x", "tmux/plugins.txt": "x" });
    assert.deepEqual(Object.keys(actions(several)), ["setup-tmux", "setup-bin", "reload"], "each part a line names whose files changed, before the reload that loads what it installs");

    const before = commit({ "setup.sh": left, "tmux/other.sh": "x" });
    commit({ "tmux/other.sh": "y" });
    assert.deepEqual(actions(before), {}, "tmux's part isn't left to do, and a script not run at load needs no reload");

    const healthy = commit({ "setup.sh": "#!/bin/sh\necho '  ○ optional: ./setup.sh claude, which replaces yours'\necho Ready.\n" });
    commit({ "claude/skills/x/SKILL.md": "x" });
    assert.deepEqual(actions(healthy), {}, "an optional part it suggests isn't left to do");

    const guided = commit({ "setup.sh": "#!/bin/sh\necho 'Left to do:'\necho \"  ✗ Switchboard's Claude Code settings have changed: ./setup.sh adds them to yours\"\nexit 1\n" });
    commit({ "claude/switchboard-settings.json": "{}" });
    const guidedListed = actions(guided);
    assert.deepEqual(Object.keys(guidedListed), ["setup-guided"], "no part but the guided setup's");
    assert.equal(guidedListed["setup-guided"][2], "", "which asks, so it is yours to run");

    const tool = commit({ "setup.sh": "#!/bin/sh\necho 'Left to do:'\necho '  ✗ fzf, which the palette needs: brew install fzf'\nexit 1\n# fzf added\n" });
    commit({ "tmux/other.sh": "z" });
    assert.deepEqual(actions(tool), {}, "a need left from before setup.sh changed is no news");
    const needs = git("rev-parse", `${tool}~1`);
    assert.deepEqual(Object.keys(actions(needs)), ["setup-guided"], "one setup.sh's change brought is");
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});

test("a repository shaped like Switchboard that isn't ~/switchboard runs none of its code to list actions", () => {
  const { tmp, commit, actions, repo } = setup();
  try {
    const ran = path.join(tmp, "ran");
    const start = commit({ "setup.sh": `#!/bin/sh\ntouch '${ran}'\n`, "bin/tmux-sidebar": "" });
    commit({ "tmux/tmux.conf": "x", "lib/sidebar/model.js": "1" });
    assert.deepEqual(actions(start), {});
    assert.equal(fs.existsSync(ran), false, `${repo}/setup.sh not run`);
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});

test("elsewhere, a changed lockfile installs where packages are installed, new migrations run the migrate script, and .env.example is yours, the scripts run only in a trusted clone", () => {
  const { tmp, home, commit, actions, repo } = setup();
  try {
    const start = commit({ "package.json": JSON.stringify({ scripts: { "db:migrate": "x" } }), "pnpm-lock.yaml": "a", "web/package-lock.json": "a", "pnp/yarn.lock": "a", "pnp/.pnp.cjs": "x" });
    fs.mkdirSync(path.join(repo, "node_modules"));
    commit({ "pnpm-lock.yaml": "b", "web/package-lock.json": "b", "pnp/yarn.lock": "b", "db/migrations/001_add.sql": "x", ".env.example": "A=1" });
    const notTrusted = actions(start);
    assert.equal(notTrusted["install:."][2], "", "a clone not in trusted-dirs runs none of its scripts");
    assert.match(notTrusted["install:."][0], /yourself \(the clone isn't in trusted-dirs\)$/);
    assert.equal(notTrusted["migrate:."][2], "");
    fs.mkdirSync(path.join(home, ".config/switchboard"), { recursive: true });
    fs.writeFileSync(path.join(home, ".config/switchboard/trusted-dirs"), `# mine\n${repo}\n`);
    const listed = actions(start);
    assert.deepEqual(Object.keys(listed), ["install:pnp", "install:.", "migrate:.", "env"], "web/ has no node_modules, so nothing there is out of date, and Yarn's Plug'n'Play counts as installed");
    assert.equal(listed["install:."][0], "Install packages with pnpm");
    assert.equal(listed["install:."][2], `cd '${repo}' && pnpm install --frozen-lockfile`);
    assert.equal(listed["migrate:."][2], `cd '${repo}' && pnpm run db:migrate`);
    assert.equal(listed.env[2], "");
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});

test("a repos.json apply list replaces the defaults, listed for its paths and run in the clone", () => {
  const { tmp, home, commit, actions, repo, git } = setup();
  try {
    git("remote", "add", "origin", "git@github.com:someone/app.git");
    fs.mkdirSync(path.join(home, ".config/switchboard"), { recursive: true });
    fs.writeFileSync(
      path.join(home, ".config/switchboard/repos.json"),
      JSON.stringify({ "someone/app": { apply: [{ paths: ["server/**"], label: "Restart the dev server", command: "make restart" }, { paths: ["docs/*.md"], label: "Rebuild the docs", command: "make docs" }, { paths: ["server/**/config.js"], label: "Reload the config", command: "make config" }] } }),
    );
    const start = commit({ "server/a.js": "1", "pnpm-lock.yaml": "a" });
    fs.mkdirSync(path.join(repo, "node_modules"));
    commit({ "server/deep/b.js": "2", "server/not-config.js": "x", "pnpm-lock.yaml": "b" });
    assert.deepEqual(actions(start), { "own:Restart the dev server": ["Restart the dev server yourself (the clone isn't in trusted-dirs)", "server/deep/b.js and 1 more changed", ""] }, "it may run the clone's Makefile");
    fs.writeFileSync(path.join(home, ".config/switchboard/trusted-dirs"), `${repo}\n`);
    assert.deepEqual(actions(start), { "own:Restart the dev server": ["Restart the dev server", "server/deep/b.js and 1 more changed", `cd '${repo}' && make restart`] });
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});

test("apply.sh reloads tmux.conf in the server $TMUX names and no other, and restarts its sidebar panes alone", () => {
  const tmp = fs.realpathSync(fs.mkdtempSync(path.join("/tmp", "apply-sh-")));
  const socket = path.join(tmp, "s");
  const env = { ...process.env, HOME: tmp, TMUX: "" };
  const tmux = (...args) => execFileSync("tmux", ["-S", socket, ...args], { env, encoding: "utf8" }).trim();
  try {
    fs.mkdirSync(path.join(tmp, ".config/tmux"), { recursive: true });
    fs.writeFileSync(path.join(tmp, ".config/tmux/tmux.conf"), "set -g @apply_test reloaded\n");
    tmux("-f", "/dev/null", "new-session", "-d");
    const pid = tmux("display-message", "-p", "#{pid}");
    execFileSync("bash", [applySh, "reload"], { env: { ...env, TMUX: `${socket},${pid},0` } });
    assert.equal(tmux("show", "-gqv", "@apply_test"), "reloaded");

    // A sidebar pane starts again with its own command; any other is left.
    const sidebar = tmux("split-window", "-d", "-P", "-F", "#{pane_id}", "sleep 300");
    tmux("set", "-p", "-t", sidebar, "@sidebar", "1");
    const before = tmux("display-message", "-p", "-t", sidebar, "#{pane_pid}");
    const notices = tmux("new-window", "-d", "-P", "-F", "#{pane_id}", "sleep 300");
    tmux("set", "-w", "-t", notices, "@notices_window", "1");
    const noticesBefore = tmux("display-message", "-p", "-t", notices, "#{pane_pid}");
    const other = tmux("display-message", "-p", "-t", "%0", "#{pane_pid}");
    execFileSync("bash", [applySh, "panes"], { env: { ...env, TMUX: `${socket},${pid},0` } });
    assert.notEqual(tmux("display-message", "-p", "-t", sidebar, "#{pane_pid}"), before);
    assert.equal(tmux("display-message", "-p", "-t", sidebar, "#{pane_start_command}"), `"sleep 300"`);
    assert.notEqual(tmux("display-message", "-p", "-t", notices, "#{pane_pid}"), noticesBefore, "the Notifications window's client too");
    assert.equal(tmux("display-message", "-p", "-t", "%0", "#{pane_pid}"), other);
  } finally {
    try {
      tmux("kill-server");
    } catch {}
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});
