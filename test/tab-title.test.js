// Tests for tmux/tab-title.sh, which shows a tab's whole title in a box (M-T,
// the sidebar's i). Run with: node --test test/*.test.js
//
// A stand-in tmux answers for the window and keeps each call, so nothing
// reaches your own tmux.

const { test } = require("node:test");
const assert = require("node:assert/strict");
const { execFileSync, spawnSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const script = path.join(__dirname, "../tmux/tab-title.sh");

test("tab-title.sh shows the whole title, wrapped, with the branch, directory, PR and ticket path-facts.sh last kept", async () => {
  const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "tab-title-")));
  try {
    const calls = path.join(tmp, "calls");
    const home = path.join(tmp, "home");
    const dir = path.join(home, "code/app");
    fs.mkdirSync(dir, { recursive: true });
    const label = `#12 AB-7: ${"a long title that the sidebar cuts at three lines ".repeat(3).trim()}`;
    // The stand-in answers display and list-panes for the window (the
    // sidebar, the notes pane beside it, then the active pane, or, with
    // NOTES_ONLY set, the sidebar and Notes alone), and keeps every call a
    // line each.
    fs.writeFileSync(path.join(tmp, "tmux"), `#!/bin/sh
printf '%s\\n' "$*" >> '${calls}'
case "$1" in
  display) printf '3\\t%s\\tapp\\n' '${label}' ;;
  list-panes) if [ -n "\${NOTES_ONLY:-}" ]; then printf '1\\t0\\t0\\t%s\\n0\\t1\\t1\\t%s\\n' '${dir}' '${home}'; else printf '1\\t0\\t0\\t%s\\n0\\t1\\t0\\t%s\\n0\\t0\\t1\\t%s\\n' '${home}' '${home}' '${dir}'; fi ;;
esac
`, { mode: 0o755 });
    const cache = path.join(tmp, "cache/tmux-path-facts");
    fs.mkdirSync(cache, { recursive: true });
    fs.writeFileSync(path.join(cache, dir.replace(/\//g, "%")), `1\tab-7-long\n#abcdef\tAB-7: a long title\t12\n`);
    const env = { ...process.env, PATH: `${tmp}:${process.env.PATH}`, HOME: home, XDG_CACHE_HOME: path.join(tmp, "cache"), XDG_STATE_HOME: tmp };

    const printed = execFileSync("bash", [script, "--print", "@3"], { env, encoding: "utf8" });
    const [title, facts] = printed.split("\n\n");
    assert.equal(title.split("\n").join(" "), label, "the whole title, nothing cut");
    assert.ok(title.split("\n").length > 1 && title.split("\n").every((line) => line.length <= 68), "wrapped to 68 columns");
    assert.deepEqual(facts.trim().split("\n"), ["Branch     ab-7-long", "Directory  ~/code/app", "PR         #12", "Ticket     AB-7"], "the active pane's, past the sidebar and Notes beside it");
    const notesTab = execFileSync("bash", [script, "--print", "@3"], { env: { ...env, NOTES_ONLY: "1" }, encoding: "utf8" });
    assert.match(notesTab, /^Directory {2}~$/m, "Notes' own tab: the notes pane's, as window-names.sh names it");

    fs.writeFileSync(calls, "");
    execFileSync("bash", [script, "-c", "client-1", "@3"], { env });
    let popup = "";
    for (let i = 0; i < 50 && !popup; i++) {
      popup = fs.readFileSync(calls, "utf8").split("\n").find((line) => line.startsWith("display-popup -c client-1 -E")) ?? "";
      if (!popup) await new Promise((resolve) => setTimeout(resolve, 50));
    }
    assert.match(popup, /-T #\[bold\] Tab 3 /, "titled with the tab's number");
    assert.match(popup, /-h 16 /, "as tall as the title's three lines, a blank, the four facts, the two options, the keys and the frame");
    assert.ok(popup.includes(`-e TAB_TITLE=${label} `), "the title in the environment, as it is");
    assert.match(fs.readFileSync(calls, "utf8"), /TAB_TITLE_FACTS=Branch[^]*Ticket     AB-7 .*tab-title\.sh box$/m, "the facts in the environment too");

    // The box itself: the title and facts, and q closes it with nothing copied.
    const shown = spawnSync("bash", [script, "box"], { env: { ...env, TAB_TITLE: label, TAB_TITLE_WRAPPED: "one\ntwo", TAB_TITLE_FACTS: "Branch     x" }, input: "q", encoding: "utf8" });
    const text = shown.stdout.replace(/\x1b\[[0-9;?]*[a-zA-Z]/g, "");
    assert.match(text, /\n {2}one\n {2}two\n\n {2}Branch {5}x\n/);
    assert.match(text, /Copy the title[^]*Close[^]*y copy/, "Copy and Close as options, y a shortcut");
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});

test("tab-title.sh shows a note item's whole first line where the tab bar has it cut short: @tab_title, else @autopilot_item, else the branch's wtnItem", async () => {
  const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "tab-title-")));
  try {
    const home = path.join(tmp, "home");
    const repo = path.join(home, "code/app");
    fs.mkdirSync(repo, { recursive: true });
    const env0 = { ...process.env, HOME: home, GIT_CONFIG_GLOBAL: "/dev/null" };
    execFileSync("git", ["init", "-q", "-b", "note/long-item", repo], { env: env0 });
    execFileSync("git", ["-C", repo, "-c", "user.name=t", "-c", "user.email=t@e", "commit", "-q", "--allow-empty", "-m", "start"], { env: env0 });
    const whole = "a tab opened for a note's item keeps only the first fifty characters as its name";
    const short = "a tab opened for a note's item keeps only the…";
    // The stand-in answers display with the fields in FIELDS, and list-panes
    // with the repository's pane.
    fs.writeFileSync(path.join(tmp, "tmux"), `#!/bin/sh
case "$1" in
  display) printf '%s\\n' "$FIELDS" ;;
  list-panes) printf '0\\t0\\t1\\t%s\\n' '${repo}' ;;
esac
`, { mode: 0o755 });
    const env = { ...env0, PATH: `${tmp}:${process.env.PATH}`, XDG_CACHE_HOME: path.join(tmp, "cache"), XDG_STATE_HOME: tmp };
    const title = (name, tabTitle, item) => execFileSync("bash", [script, "--print", "@3"], { env: { ...env, FIELDS: ["3", `#12 ${name}`, "app", name, tabTitle, item].join("\t") }, encoding: "utf8" }).split("\n\n")[0].split("\n").join(" ");

    assert.equal(title(short, whole, "none"), `#12 ${whole}`, "@tab_title, the PR number in front");
    assert.equal(title(short, "none", whole), `#12 ${whole}`, "a tab opened before @tab_title: the mode's item");
    assert.equal(title(whole.slice(0, 50), "none", whole), `#12 ${whole}`, "a name cut mid-word, as tabs were named before");
    assert.equal(title("Renamed", whole, whole), "#12 Renamed", "renamed since: its new name");
    assert.equal(title("a tab opened", "none", whole), "#12 a tab opened", "renamed to the start of the item: still its new name");
    assert.equal(title(short, "none", "none"), `#12 ${short}`, "nothing kept: the name");
    execFileSync("git", ["-C", repo, "config", "branch.note/long-item.wtnItem", `Work/Notes.md: ${whole}`], { env: env0 });
    assert.equal(title(short, "none", "none"), `#12 ${whole}`, "the item its branch is for");
    execFileSync("git", ["-C", repo, "config", "branch.note/long-item.wtnItem", `n-0123456789ab: ${whole}`], { env: env0 });
    assert.equal(title(short, "none", "none"), `#12 ${whole}`, "a claim by the note's ID too");
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});
