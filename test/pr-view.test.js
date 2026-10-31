// M-v's and M-i's views draw the copy last read at once and read it again in
// the background, drawing it again when it changed. Run in a throwaway tmux
// server with stand-ins for gh and Linear's API, and a temporary cache and
// settings.

const { test, after } = require("node:test");
const assert = require("node:assert");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const http = require("node:http");
const { execFileSync } = require("node:child_process");

// Under /tmp: a Unix socket's path can't be over 104 bytes on macOS.
const tmp = fs.mkdtempSync(path.join("/tmp", "pr-view-"));
const socket = path.join(tmp, "tmux.sock");
const tmux = (...args) => execFileSync("tmux", ["-S", socket, "-f", "/dev/null", ...args], { encoding: "utf8" }).trim();
const repo = path.join(tmp, "repo");
const bin = path.join(tmp, "bin");
const body = path.join(tmp, "body");
const scripts = path.join(__dirname, "..", "tmux");
fs.mkdirSync(bin);
execFileSync("git", ["init", "-q", repo]);
execFileSync("git", ["-C", repo, "commit", "-q", "--allow-empty", "-m", "first"]);
// Each answers with what the body file says now, after a second, as the network would.
fs.writeFileSync(
  path.join(bin, "gh"),
  `#!/bin/sh\nsleep 1\nprintf '{"number":7,"title":"T","url":"https://github.com/o/r/pull/7","state":"OPEN","isDraft":false,"author":{"login":"a"},"baseRefName":"main","headRefName":"x","body":"%s"}' "$(cat '${body}')"\n`,
  { mode: 0o755 },
);
// Linear, as lib/tracker asks it through tmux/tracker.sh.
const linear = http.createServer((req, res) => {
  req.resume();
  req.on("end", () =>
    setTimeout(() => {
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(JSON.stringify({ data: { issue: { identifier: "AB-1", title: "T", url: "https://linear.app/x/issue/AB-1", description: fs.readFileSync(body, "utf8"), createdAt: "2026-10-01T00:00:00Z", updatedAt: "2026-10-01T00:00:00Z", state: { name: "Todo" }, labels: { nodes: [] }, children: { nodes: [] }, relations: { nodes: [] }, attachments: { nodes: [] }, comments: { nodes: [] } } } }));
    }, 1_000),
  );
});
const listening = new Promise((resolve) => linear.listen(0, "127.0.0.1", resolve));

after(() => {
  linear.close();
  try {
    tmux("kill-server");
  } catch {}
  fs.rmSync(tmp, { recursive: true, force: true });
});

const views = [
  { name: "the PR view", command: `bash ${scripts}/pr-view.sh show`, cache: "pr-view" },
  { name: "the ticket view", command: `bash ${scripts}/ticket-view.sh show AB-1`, cache: "ticket-view" },
];

views.forEach((view) => {
  test(`${view.name} draws the last copy at once, then the new one`, async () => {
    await listening;
    const cache = path.join(tmp, `cache-${view.cache}`);
    const linearEnv = `LINEAR_API_KEY=k LINEAR_API_URL=http://127.0.0.1:${linear.address().port} SWITCHBOARD_DIR=${path.join(__dirname, "..")} SWITCHBOARD_SETTINGS=${path.join(tmp, "settings.json")}`;
    const open = () =>
      tmux("new-session", "-d", "-s", view.cache, "-x", "100", "-y", "30", "-c", repo, `env PATH=${bin}:${process.env.PATH} ${linearEnv} XDG_CACHE_HOME=${cache} ${view.command}`);
    const shows = async (text) => {
      const until = Date.now() + 10_000;
      while (Date.now() < until) {
        if (tmux("capture-pane", "-p", "-t", view.cache).includes(text)) return true;
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
      return false;
    };
    const close = async () => {
      tmux("send-keys", "-t", view.cache, "q");
      const until = Date.now() + 5_000;
      while (Date.now() < until) {
        try {
          tmux("has-session", "-t", view.cache);
        } catch {
          return;
        }
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
      assert.fail("q didn't close it");
    };

    fs.writeFileSync(body, "first body");
    open();
    assert.ok(await shows("first body"), "read and drawn the first time");
    await close();
    assert.ok(fs.readdirSync(path.join(cache, "switchboard", view.cache)).length, "kept");

    fs.writeFileSync(body, "second body");
    const opened = Date.now();
    open();
    assert.ok(await shows("first body"));
    assert.ok(Date.now() - opened < 1_000, "the last copy before the network answers");
    assert.ok(await shows("second body"), "drawn again once read");
    await close();
  });
});
