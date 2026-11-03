// Tests for tmux/link-view.sh's picker. Run with: node --test test/*.test.js
//
// It reads a pane of a throwaway tmux server, whose socket is in a temporary
// directory under /tmp (a socket's path can't be over 104 bytes on macOS), and
// fzf is a stand-in that keeps what it was given and picks nothing, so nothing
// is opened.

const { test, after } = require("node:test");
const assert = require("node:assert/strict");
const { execFileSync, spawnSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");

const tmp = fs.realpathSync(fs.mkdtempSync("/tmp/link-view-"));
const socket = path.join(tmp, "tmux.sock");
const env = { ...process.env, HOME: tmp, TMUX: "" };
after(() => {
  spawnSync("tmux", ["-S", socket, "kill-server"], { env });
  fs.rmSync(tmp, { recursive: true, force: true });
});

test("the picker lists a pane's links newest first, a link written into text with its text", () => {
  const screen = path.join(tmp, "screen.txt");
  fs.writeFileSync(
    screen,
    [
      "See https://plain.example/one.",
      "\x1b[1mThe \x1b]8;;https://linear.app/team/issue/ABC-1\x1b\\\x1b[32mticket title\x1b[0m\x1b]8;;\x1b\\ is linked",
      "again https://plain.example/one and \x1b]8;;https://github.com/o/r/pull/2\x07the PR\x1b]8;;\x07",
      "\x1b]8;;https://same.example/x\x1b\\https://same.example/x\x1b]8;;\x1b\\",
      "",
    ].join("\n"),
  );
  execFileSync("tmux", ["-S", socket, "-f", "/dev/null", "new-session", "-d", "-x", "200", "-y", "20", `cat ${screen}; sleep 60`], { env });
  const bin = path.join(tmp, "bin");
  fs.mkdirSync(bin);
  const given = path.join(tmp, "fzf-input");
  fs.writeFileSync(path.join(bin, "fzf"), `#!/bin/sh\ncat > ${given}\nexit 1\n`, { mode: 0o755 });
  spawnSync("sleep", ["0.5"]);
  const pane = execFileSync("tmux", ["-S", socket, "display", "-p", "#{pane_id}"], { env, encoding: "utf8" }).trim();

  const run = spawnSync("bash", [path.join(__dirname, "../tmux/link-view.sh"), "pick", pane], {
    env: { ...env, TMUX: `${socket},0,0`, PATH: `${bin}:${process.env.PATH}`, LC_ALL: "en_US.UTF-8" },
    encoding: "utf8",
  });
  assert.equal(run.status, 0, run.stderr);
  const rows = fs.readFileSync(given, "utf8").trim().split("\n").map((row) => row.replace(/\x1b\[[0-9;]*m/g, "").split("\t"));
  assert.deepEqual(rows, [
    ["https://same.example/x", "https://same.example/x"],
    ["the PR  https://github.com/o/r/pull/2", "https://github.com/o/r/pull/2"],
    ["https://plain.example/one", "https://plain.example/one"],
    ["ticket title  https://linear.app/team/issue/ABC-1", "https://linear.app/team/issue/ABC-1"],
  ]);
});

test("the picker's copy mode copies the chosen link to the clipboard and says so", () => {
  const pane = execFileSync("tmux", ["-S", socket, "display", "-p", "#{pane_id}"], { env, encoding: "utf8" }).trim();
  // fzf picks the PR's row with Enter; pbcopy keeps what it was given.
  const bin = path.join(tmp, "bin-copy");
  fs.mkdirSync(bin, { recursive: true });
  const copied = path.join(tmp, "copied");
  fs.writeFileSync(path.join(bin, "fzf"), "#!/bin/sh\ncat > /dev/null\nprintf '\\nthe PR\\thttps://github.com/o/r/pull/2\\n'\n", { mode: 0o755 });
  fs.writeFileSync(path.join(bin, "pbcopy"), `#!/bin/sh\ncat > ${copied}\n`, { mode: 0o755 });
  const run = spawnSync("bash", [path.join(__dirname, "../tmux/link-view.sh"), "pick", pane, "copy"], {
    env: { ...env, TMUX: `${socket},0,0`, PATH: `${bin}:${process.env.PATH}`, LC_ALL: "en_US.UTF-8" },
    encoding: "utf8",
  });
  assert.equal(run.status, 0, run.stderr);
  assert.equal(fs.readFileSync(copied, "utf8"), "https://github.com/o/r/pull/2");
});

test("in the notes pane, the picker lists the open note's links, top to bottom, since Notes hides their addresses", () => {
  const notes = path.join(tmp, "NotesForLinks");
  fs.mkdirSync(notes, { recursive: true });
  fs.writeFileSync(path.join(notes, "Plan.md"), "# Plan\n\n- read [the PR](https://github.com/o/r/pull/3) first\n- then https://plain.example/two.\n- again [same](https://github.com/o/r/pull/3)\n");
  fs.writeFileSync(path.join(notes, ".notes.json"), JSON.stringify({ last: "Plan.md" }));
  execFileSync("tmux", ["-S", socket, "-f", "/dev/null", "new-session", "-d", "-s", "notes", "-x", "200", "-y", "20", "echo the PR; sleep 60"], { env });
  const pane = execFileSync("tmux", ["-S", socket, "display", "-p", "-t", "notes", "#{pane_id}"], { env, encoding: "utf8" }).trim();
  const links = () =>
    spawnSync("bash", [path.join(__dirname, "../tmux/link-view.sh"), "links", pane], {
      env: { ...env, TMUX: `${socket},0,0`, NOTES_DIR: notes, LC_ALL: "en_US.UTF-8" },
      encoding: "utf8",
    });
  const plainPane = links();
  assert.equal(plainPane.stdout.trim(), "", "an ordinary pane showing no address has no links");
  assert.match(plainPane.stderr, /No links in this pane/);
  execFileSync("tmux", ["-S", socket, "set", "-p", "-t", pane, "@notes_pane", "1"], { env });
  const notesPane = links();
  assert.equal(notesPane.status, 0, notesPane.stderr);
  assert.deepEqual(notesPane.stdout.trim().split("\n").map((row) => row.replace(/\x1b\[[0-9;]*m/g, "").split("\t")), [
    ["the PR  https://github.com/o/r/pull/3", "https://github.com/o/r/pull/3"],
    ["https://plain.example/two", "https://plain.example/two"],
  ]);
});

test("the media picker lists the images and videos a pane shows, newest first, files only where they are, and opens the chosen one", () => {
  const work = path.join(tmp, "media-wörk");
  fs.mkdirSync(path.join(work, "shots"), { recursive: true });
  fs.writeFileSync(path.join(work, "shots", "a.png"), "");
  fs.writeFileSync(path.join(work, "shots", "b.png"), "");
  fs.writeFileSync(path.join(work, "shots", "café.png"), "");
  fs.writeFileSync(path.join(work, "shots", "bob's.png"), "");
  fs.writeFileSync(path.join(tmp, "clip.mp4"), "");
  const shown = path.join(tmp, "media-screen.txt");
  fs.writeFileSync(
    shown,
    [
      "saved ./shots/a.png and image=~/clip.mp4 and ~/clip.mp4, gone.png is not there, nor shots/b.png.bak or shots/b.png_backup or shots/b.png.~1~ or #shots/b.png# or shots/b.png$x or c.png$shots/b.png or sam's/b.png, but shots/bob's.png and shots/café.png, while '~/clip.mp4' is",
      "see https://example.com/x/pic.jpg?raw=1 and (shots/a.png), not https://example.com/assets.png/page nor https://studio.mov",
      "\x1b]8;;https://example.com/clip.mov#t=30\x1b\\the recording\x1b]8;;\x1b\\",
      "",
    ].join("\n"),
  );
  execFileSync("tmux", ["-S", socket, "-f", "/dev/null", "new-session", "-d", "-s", "media", "-x", "200", "-y", "20", "-c", work, `cat ${shown}; sleep 60`], { env });
  spawnSync("sleep", ["0.5"]);
  const pane = execFileSync("tmux", ["-S", socket, "display", "-p", "-t", "media", "#{pane_id}"], { env, encoding: "utf8" }).trim();
  const run = (args, extra = {}) => spawnSync("bash", [path.join(__dirname, "../tmux/link-view.sh"), ...args, pane], {
    env: { ...env, TMUX: `${socket},0,0`, LC_ALL: "en_US.UTF-8", ...extra },
    encoding: "utf8",
  });
  const listed = run(["media-list"]);
  assert.equal(listed.status, 0, listed.stderr);
  assert.deepEqual(listed.stdout.trim().split("\n").map((row) => row.split("\t")[1]), [
    path.join(work, "shots", "a.png"),
    path.join(tmp, "clip.mp4"),
    path.join(work, "shots", "café.png"),
    path.join(work, "shots", "bob's.png"),
    "https://example.com/clip.mov#t=30",
    "https://example.com/x/pic.jpg?raw=1",
  ], "files newest first, one named twice once, a missing one left out; then the links to media, whole, a hyperlink's too, and not a page under a .png directory");

  // fzf picks the video; open (a stand-in) is given it.
  const bin = path.join(tmp, "bin-media");
  fs.mkdirSync(bin, { recursive: true });
  const opened = path.join(tmp, "opened");
  fs.writeFileSync(path.join(bin, "fzf"), `#!/bin/sh\ngrep clip.mp4 | sed '1s/^/\\n/'\n`, { mode: 0o755 });
  fs.writeFileSync(path.join(bin, "open"), `#!/bin/sh\necho "$@" > ${opened}\n`, { mode: 0o755 });
  const picked = run(["media"], { PATH: `${bin}:${process.env.PATH}` });
  assert.equal(picked.status, 0, picked.stderr);
  assert.equal(fs.readFileSync(opened, "utf8").trim(), path.join(tmp, "clip.mp4"));
});

test("search everything finds the notes holding every word, newest first, as links the link view reads, and nothing in .trash", () => {
  const notes = path.join(tmp, "Notes");
  fs.mkdirSync(path.join(notes, "Work"), { recursive: true });
  fs.mkdirSync(path.join(notes, ".trash"));
  fs.writeFileSync(path.join(notes, "Old plan.md"), "# Old plan\n\n- [ ] deploy the freeze fix\n");
  fs.writeFileSync(path.join(notes, "Work", "Carl's (draft).md"), "# Carl's (draft)\n\nThe Deploy freeze is on\n");
  fs.writeFileSync(path.join(notes, "Other.md"), "# Other\n\ndeploy only\n");
  fs.writeFileSync(path.join(notes, ".trash", "Gone.md"), "# Gone\n\ndeploy freeze\n");
  fs.utimesSync(path.join(notes, "Old plan.md"), new Date(2020, 0, 1), new Date(2020, 0, 1));
  execFileSync("tmux", ["-S", socket, "-f", "/dev/null", "new-session", "-d", "-s", "search", "sleep 60"], { env });
  ["github", "linear", "search_slack"].forEach((key) => execFileSync("tmux", ["-S", socket, "set", "-g", `@setting_${key}`, "off"], { env }));

  const out = execFileSync("node", [path.join(__dirname, "../lib/search.js"), "deploy", "FREEZE"], {
    env: { ...env, TMUX: `${socket},0,0`, NOTES_DIR: notes },
    encoding: "utf8",
  });
  const rows = out.trim().split("\n").map((row) => row.replace(/\x1b\[[0-9;]*m/g, "").split("\t"));
  assert.deepEqual(rows, [
    ["Note   Carl's (draft)  The Deploy freeze is on", `file://${notes}/Work/Carl%27s%20%28draft%29.md`],
    ["Note   Old plan  deploy the freeze fix", `file://${notes}/Old%20plan.md`],
  ]);
  assert.match(rows[0][1], /^file:\/\/[A-Za-z0-9._~:/?#@!$&()*+,;=%-]+$/, "a link the link view takes");
});
