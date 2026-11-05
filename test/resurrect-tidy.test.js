// Tests for tmux/resurrect-tidy.sh, which keeps tmux-resurrect's saves
// private and few. Run with: node --test test/*.test.js
//
// It runs against a throwaway tmux server and a temporary HOME, so it never
// touches your tmux or your saves.

const { test, after } = require("node:test");
const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "tidy-")));
const socket = path.join(tmp, "tmux.sock");
execFileSync("tmux", ["-S", socket, "-f", "/dev/null", "new-session", "-d", "cat"]);
after(() => {
  execFileSync("tmux", ["-S", socket, "kill-server"]);
  fs.rmSync(tmp, { recursive: true, force: true });
});

test("tidies the folder resurrect saves to, its @resurrect-dir when set, and no other", () => {
  // The custom folder named through a link, as one kept in a synced folder may be.
  const custom = path.join(tmp, "saves-real");
  const xdg = path.join(tmp, "data/tmux/resurrect");
  [custom, xdg].forEach((dir) => fs.mkdirSync(dir, { recursive: true, mode: 0o755 }));
  fs.symlinkSync(custom, path.join(tmp, "saves"));
  const names = Array.from({ length: 55 }, (_, i) => `tmux_resurrect_20261104T${String(100000 + i)}.txt`);
  names.forEach((name) => {
    fs.writeFileSync(path.join(custom, name), "", { mode: 0o644 });
    fs.writeFileSync(path.join(xdg, name), "", { mode: 0o644 });
  });
  // The oldest is the one `last` points at, so it is kept whatever its age.
  fs.symlinkSync(names[0], path.join(custom, "last"));
  execFileSync("tmux", ["-S", socket, "set", "-g", "@resurrect-dir", "~/saves"]);

  execFileSync("bash", [path.join(__dirname, "../tmux/resurrect-tidy.sh")], {
    env: { ...process.env, HOME: tmp, XDG_DATA_HOME: path.join(tmp, "data"), TMUX: `${socket},0,0` },
  });

  const left = fs.readdirSync(custom).filter((n) => n.startsWith("tmux_resurrect_"));
  assert.equal(left.length, 51, "the newest 50, and the one last points at");
  assert.ok(left.includes(names[0]));
  assert.equal(fs.statSync(custom).mode & 0o777, 0o700, "owner-only");
  assert.equal(fs.statSync(path.join(custom, names[54])).mode & 0o777, 0o600, "its saves too, through the link");
  assert.equal(fs.readdirSync(xdg).length, 55, "the default folder, which resurrect doesn't save to here, is left alone");
});
