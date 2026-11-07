const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { spawnSync } = require("node:child_process");

const script = path.join(__dirname, "..", "tmux", "palette.sh");

test("palette.sh --rank puts first the command picked after the same words, then one picked after longer ones", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "palette-"));
  try {
    const rows = path.join(dir, "rows");
    // A backslash in the path, which awk must take as written.
    fs.mkdirSync(path.join(dir, "state\\notes"));
    const queries = path.join(dir, "state\\notes", "queries");
    fs.writeFileSync(rows, "a\tOpen the PR #github\nb\tShape a note #notes\nc\tSearch everything #search\n");
    // b picked three times after shape, c once after sh itself.
    fs.writeFileSync(queries, "sh\tc\nshape\tb\nshape\tb\nshape\tb\n");
    const rank = (...args) => spawnSync("bash", [script, "--rank", rows, ...args], { encoding: "utf8" }).stdout.trim().split("\n").map((row) => row.split("\t")[0]);

    assert.deepEqual(rank("sh", queries), ["c", "b", "a"], "picked after sh itself, however often the other was after shape");
    assert.deepEqual(rank("  SHAP ", queries), ["b", "a", "c"], "case and spaces don't matter, and shape starts with shap");
    assert.deepEqual(rank("pr", queries), ["a", "b", "c"], "nothing learned for pr: the words decide");
    assert.deepEqual(rank("sh", ""), ["b", "a", "c"], "with learning off, the words alone");
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("palette.sh --filter lists a #diffview command only while the pane shows M-d's diff, and Ask only with diff_ask on", () => {
  const rows = "M-p\tShow this list #palette\n@palette D\tCopy the diff's story #story #diffview\n@palette k\tAsk Claude about the diff #ai #diffview\nM-r\tOpen the PR #github\n";
  const keys = (...args) => spawnSync("bash", [script, "--filter", ...args], { input: rows, encoding: "utf8" }).stdout.trim().split("\n").filter(Boolean).map((row) => row.split("\t")[0]);

  assert.deepEqual(keys("on", "on", "diffview", "on"), ["@palette D", "@palette k", "M-r"], "with the diff open, both of its commands");
  assert.deepEqual(keys("on", "on", "", "on"), ["M-r"], "in any other pane, neither");
  assert.deepEqual(keys("on", "on", "lazygit", "on"), ["M-r"], "nor with another tool in its place");
  assert.deepEqual(keys("on", "on", "diffview", "off"), ["@palette D", "M-r"], "Ask left out with diff_ask off");
  assert.deepEqual(keys("off", "on", "diffview", "on"), ["@palette D", "@palette k"], "#github still follows its setting");
});

test("palette.sh --filter keeps M-p itself for the Keys view's list (all), which the palette leaves out", () => {
  const rows = "M-p\tShow this list #palette\nM-r\tOpen the PR #github\n";
  const keys = (...args) => spawnSync("bash", [script, "--filter", ...args], { input: rows, encoding: "utf8" }).stdout.trim().split("\n").filter(Boolean).map((row) => row.split("\t")[0]);
  assert.deepEqual(keys("on", "on", "", "on"), ["M-r"]);
  assert.deepEqual(keys("on", "on", "", "on", "all"), ["M-p", "M-r"]);
});
