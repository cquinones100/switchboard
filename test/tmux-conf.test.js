// Tests that tmux/tmux.conf loads as written. Run with: node --test test/*.test.js
//
// It is loaded into a throwaway tmux server with @conf_check set, which skips
// what would reach beyond that server (starting the sidebar daemon, the
// plugins, the theme), and HOME is a temporary directory, so nothing touches
// your tmux, your saved sessions or your sidebar.

const { test, after } = require("node:test");
const assert = require("node:assert/strict");
const { execFileSync, spawnSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "tmux-conf-")));
const socket = path.join(tmp, "tmux.sock");
const env = { ...process.env, HOME: tmp, TMUX: "" };
const conf = path.join(__dirname, "../tmux/tmux.conf");
execFileSync("tmux", ["-S", socket, "-f", "/dev/null", "new-session", "-d", "-s", "conf", "cat"], { env });
const tmux = (...args) => spawnSync("tmux", ["-S", socket, ...args], { encoding: "utf8", env });
after(() => {
  tmux("kill-server");
  fs.rmSync(tmp, { recursive: true, force: true });
});

// tmux's own word splitting, as far as tmux.conf needs it: quotes, a
// backslash escaping the next character outside single quotes, and `\;`, the
// command separator, as a word of its own.
const words = (line) => {
  const out = [];
  let word = "";
  let quote = "";
  let started = false;
  for (let i = 0; i < line.length; i++) {
    const ch = line[i];
    if (quote) {
      if (ch === quote) quote = "";
      else if (ch === "\\" && quote === '"') word += line[++i];
      else word += ch;
    } else if (ch === "'" || ch === '"') {
      quote = ch;
      started = true;
    } else if (ch === "\\" && line[i + 1] === ";" && !started) {
      out.push("\\;");
      i++;
    } else if (ch === "\\") {
      word += line[++i];
      started = true;
    } else if (/\s/.test(ch)) {
      if (started) out.push(word);
      word = "";
      started = false;
    } else {
      word += ch;
      started = true;
    }
  }
  if (started) out.push(word);
  return out;
};

test("tmux.conf loads without an error, and every binding keeps all its commands", () => {
  assert.equal(tmux("set", "-g", "@conf_check", "1").status, 0);
  const loaded = tmux("source-file", conf);
  assert.equal(loaded.status, 0, loaded.stderr || loaded.stdout);
  assert.equal(loaded.stderr, "");

  // Each bind in the file: its table, its key, and how many commands it runs
  // (a lost backslash before ; ends the bind early and runs the rest once, at
  // load, which is how M-Tab and the flag action broke).
  const lines = fs.readFileSync(conf, "utf8").replace(/\\\n/g, " ").split("\n");
  const binds = lines
    .filter((line) => /^\s*bind(-key)? /.test(line))
    .map((line) => {
      const w = words(line.trim()).slice(1);
      let table = "prefix";
      while (w[0]?.startsWith("-")) {
        const flag = w.shift();
        if (flag === "-n") table = "root";
        if (flag === "-T") table = w.shift();
        if (flag === "-N") w.shift();
      }
      // A bare ; counts too: tmux reads it as the end of the bind.
      return { table, key: w[0], commands: w.slice(1).filter((x) => x === "\\;" || x === ";").length + 1, line };
    });
  assert.ok(binds.length > 30, "the binds were read");
  // A key bound twice in one table keeps only its last binding, silently.
  const seen = new Map();
  binds.forEach((bind) => {
    const id = `${bind.table} ${bind.key}`;
    assert.ok(!seen.has(id), `${id} is bound twice:\n${seen.get(id)}\n${bind.line}`);
    seen.set(id, bind.line);
  });

  const listed = tmux("list-keys").stdout.split("\n").filter(Boolean).map((line) => {
    const w = words(line).slice(1);
    while (w[0] === "-r") w.shift();
    assert.equal(w.shift(), "-T");
    const table = w.shift();
    return { table, key: w.shift(), commands: w.filter((x) => x === "\\;").length + 1 };
  });
  binds.forEach((bind) => {
    const found = listed.find((l) => l.table === bind.table && l.key === bind.key);
    assert.ok(found, `not bound: ${bind.table} ${bind.key}\n${bind.line}`);
    assert.equal(found.commands, bind.commands, `${bind.table} ${bind.key} runs ${found.commands} command(s), the file gives it ${bind.commands}\n${bind.line}`);
  });
});

test("M-arrows move between panes, each listed in M-p, and the Commands button shows only with its setting on", () => {
  assert.equal(tmux("set", "-g", "@conf_check", "1").status, 0);
  assert.equal(tmux("source-file", conf).status, 0);
  const notes = tmux("list-keys", "-N", "-T", "root").stdout;
  ["M-Left", "M-Down", "M-Up", "M-Right"].forEach((key) => {
    assert.match(notes, new RegExp(`^\\S+ ${key} +Move to the pane .*#panes #navigate$`, "m"), `${key} has a description for M-p`);
  });
  assert.match(tmux("list-keys", "-T", "root", "M-Left").stdout, /select-pane -L/);

  assert.equal(tmux("set", "-gu", "@setting_palette_button").status, 0);
  assert.match(tmux("display", "-p", "#{E:status-left}").stdout, /range=user\|palette.*⌘ Commands/);
  assert.equal(tmux("set", "-g", "@setting_palette_button", "off").status, 0);
  const off = tmux("display", "-p", "#{E:status-left}").stdout;
  assert.doesNotMatch(off, /palette|Commands/);
  assert.match(off, / conf /, "the rest of the status line stays");
  assert.match(tmux("list-keys", "-T", "root", "MouseDown1Status").stdout, /mouse_status_range.*palette/);
});

test("every described root key, and M-2 to M-9, counts itself in @used_keys", () => {
  assert.equal(tmux("set", "-g", "@conf_check", "1").status, 0);
  assert.equal(tmux("source-file", conf).status, 0);
  const notes = tmux("list-keys", "-N", "-T", "root").stdout.split("\n").filter(Boolean).map((line) => line.split(/\s+/)[1]);
  const keys = [...new Set([...notes, "M-2", "M-3", "M-4", "M-5", "M-6", "M-7", "M-8", "M-9"])];
  assert.ok(keys.length > 30, "the described keys were read");
  keys.forEach((key) => {
    const bound = tmux("list-keys", "-T", "root", key).stdout;
    assert.ok(bound.includes(`set-option -ag @used_keys ${key},`) || bound.includes(`set-option -ag @used_keys "${key},"`) || bound.includes(`set-option -ag @used_keys '${key},'`), `${key} starts with used '${key},'\n${bound}`);
  });
  assert.equal(tmux("set", "-gu", "@used_keys").status, 0);
  tmux("used", "M-Tab,");
  tmux("used", "M-d,");
  assert.equal(tmux("show", "-gqv", "@used_keys").stdout.trim(), "M-Tab,M-d,");
});
