// bin/leak-check: what it finds in a range's added lines and commit
// messages, against a throwaway repository and HOME.

const { test } = require("node:test");
const assert = require("node:assert");
const { execFileSync, spawnSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const SCRIPT = path.join(__dirname, "../bin/leak-check");
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "leak-check-"));
const home = path.join(tmp, "home");
const repo = path.join(tmp, "repo");
const env = { ...process.env, HOME: home, GIT_CONFIG_GLOBAL: "/dev/null", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e" };
const git = (...args) => execFileSync("git", args, { cwd: repo, env, encoding: "utf8" }).trim();

fs.mkdirSync(path.join(home, ".config/switchboard"), { recursive: true });
fs.mkdirSync(repo, { recursive: true });
git("init", "-q", "-b", "main");
fs.writeFileSync(path.join(repo, "a.md"), "Ticket FOO-9 and #4567 were here.\n");
git("add", ".");
git("commit", "-q", "-m", "start");
const base = git("rev-parse", "HEAD");

// Commits `files` with `message` and returns leak-check's run on base..HEAD.
const check = (files, message = "Add a thing") => {
  git("reset", "-q", "--hard", base);
  for (const [file, text] of Object.entries(files)) {
    fs.mkdirSync(path.dirname(path.join(repo, file)), { recursive: true });
    if (text === null) fs.rmSync(path.join(repo, file));
    else fs.writeFileSync(path.join(repo, file), text);
  }
  git("add", "-A");
  git("commit", "-q", "--allow-empty", "-m", message);
  const run = spawnSync(SCRIPT, [`${base}..HEAD`], { cwd: repo, env, encoding: "utf8" });
  return { code: run.status, out: run.stdout.trim(), err: run.stderr.trim() };
};

// Token shapes, a home path and an address, put together here so this file
// holds none whole, and passes leak-check itself.
const token = (prefix) => `${prefix}${"a1B2".repeat(8)}`;
const HOME_PATH = ["", "Users", "somebody"].join("/");
const EMAIL = ["jane.doe", "company.io"].join("@");

test("each kind is caught in an added line, with its file and line", () => {
  const cases = [
    ["Fixed in ENG-123 today", /ticket ID: ENG-123/],
    ["See #4321 for why", /PR number: #4321/],
    ["Related PR: #4321", /PR number: #4321/],
    ["Follow up PLATFORM-42", /ticket ID: PLATFORM-42/],
    ["Follow up A-42", /ticket ID: A-42/],
    ["Follow up B2B-42", /ticket ID: B2B-42/],
    ["git checkout ENG-456-fix-login", /ticket ID: ENG-456/],
    [`Contact ${["me", "company.io"].join("@")}`, /email address: me@company\.io/],
    ['{ "pr": "#456" }', /PR number: #456/],
    ["Fix background polling from #456", /PR number: #456/],
    [`cd ${HOME_PATH}/code`, /home path: \/Users\/somebody/],
    [`Mail ${EMAIL}`, /email address: jane\.doe@company\.io/],
    [`key ${token("ghp_")}`, /token: ghp_a1B2/],
    [`key ${token("sk-")}`, /token: sk-a1B2a/],
    [`key ${token("xoxb-")}`, /token: xoxb-a1B/],
    [`key ${token("xapp-")}`, /token: xapp-a1B/],
  ];
  for (const [line, expected] of cases) {
    const { code, out } = check({ "lib/x.js": `one\n// ${line}\n` });
    assert.equal(code, 1, line);
    assert.match(out, new RegExp(`^lib/x\\.js:2: ${expected.source}`), line);
  }
});

test("each kind is caught in a commit message", () => {
  const { code, out } = check({}, `Fix ENG-77\n\nAs #9876 asked, for ${EMAIL} in ${HOME_PATH}`);
  assert.equal(code, 1);
  for (const kind of ["ticket ID: ENG-77", "PR number: #9876", `email address: ${EMAIL}`, `home path: ${HOME_PATH}`]) assert.match(out, new RegExp(`^commit [0-9a-f]+: ${kind}`, "m"), kind);
});

test("a word from leak-check.txt is caught as a whole word, any case", () => {
  fs.writeFileSync(path.join(home, ".config/switchboard/leak-check.txt"), "# my words\n\nAcmeCorp\nintranet.acme.test\n");
  try {
    const { code, out } = check({ "notes.md": "Built for acmecorp at intranet.acme.test\nNot acmecorporation\n" });
    assert.equal(code, 1);
    assert.deepEqual(out.split("\n"), ["notes.md:1: listed word: acmecorp", "notes.md:1: listed word: intranet.acme.test"]);
  } finally {
    fs.rmSync(path.join(home, ".config/switchboard/leak-check.txt"));
  }
});

test("a line one commit adds and a later one removes still counts, since the push carries both", () => {
  git("reset", "-q", "--hard", base);
  fs.writeFileSync(path.join(repo, "b.md"), `key ${token("ghp_")}\n`);
  git("add", "-A");
  git("commit", "-q", "-m", "Add b");
  fs.rmSync(path.join(repo, "b.md"));
  git("add", "-A");
  git("commit", "-q", "-m", "Remove b");
  const run = spawnSync(SCRIPT, [`${base}..HEAD`], { cwd: repo, env, encoding: "utf8" });
  assert.equal(run.status, 1);
  assert.match(run.stdout, /^b\.md:1: token: ghp_a1B2/);
});

test("a side branch merged in counts too, its added line removed before the merge included", () => {
  git("reset", "-q", "--hard", base);
  git("checkout", "-q", "-b", "side");
  fs.writeFileSync(path.join(repo, "c.md"), `key ${token("ghp_")}\n`);
  git("add", "-A");
  git("commit", "-q", "-m", "Add c");
  fs.rmSync(path.join(repo, "c.md"));
  git("add", "-A");
  git("commit", "-q", "-m", "Remove c");
  git("checkout", "-q", "main");
  git("reset", "-q", "--hard", base);
  git("commit", "-q", "--allow-empty", "-m", "Main moves");
  git("merge", "-q", "--no-ff", "-m", "Merge side", "side");
  git("branch", "-q", "-D", "side");
  const run = spawnSync(SCRIPT, [`${base}..HEAD`], { cwd: repo, env, encoding: "utf8" });
  assert.equal(run.status, 1);
  assert.match(run.stdout, /^c\.md:1: token: ghp_a1B2/m);
});

test("a file the repository marks binary is still scanned", () => {
  const { code, out } = check({ ".gitattributes": ".env -diff\n", ".env": `TOKEN=${token("ghp_")}\n` });
  assert.equal(code, 1);
  assert.match(out, /^\.env:1: token: ghp_a1B2/m);
});

test("removed lines are left out, as is what was there before the range", () => {
  assert.deepEqual(check({ "a.md": null }), { code: 0, out: "", err: "" });
});

test("made-up IDs are allowed in tests, and the docs' placeholders anywhere", () => {
  assert.equal(check({ "test/x.test.js": "assert(ticket === 'ENG-123' && pr === '#4321')\n" }).code, 0, "a test's IDs are made up");
  assert.equal(check({ "lib/x.test.js": "const id = 'FOO-1';\n" }).code, 0, "a .test. file too");
  assert.equal(check({ "README.md": "Like ABC-123, #123 or #12345, in UTF-8 with SHA-256, MPL-2.0, M-1 and C-2\n" }).code, 0, "placeholders and standards");
  assert.equal(check({ "x.css": ".a { color: #202124; } .b { background: \"#000000\" }\n", "x.lua": "local bg = \"#000000\"\n" }).code, 0, "colors");
  assert.equal(check({ "x.md": "git@github.com:me/x and noreply@anthropic.com\n" }).code, 0, "SSH and noreply addresses");
  assert.equal(check({}, "Add a thing\n\nCo-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>").code, 0, "Claude's trailer");
  assert.equal(check({ "test/x.test.js": `cd ${HOME_PATH}\n` }).code, 1, "a home path counts in a test too");
});

test("exits 2 when it can't look", () => {
  const usage = spawnSync(SCRIPT, [], { cwd: repo, env, encoding: "utf8" });
  assert.equal(usage.status, 2);
  fs.mkdirSync(path.join(home, ".config/switchboard/leak-check.txt"));
  const unreadable = spawnSync(SCRIPT, ["HEAD..HEAD"], { cwd: repo, env, encoding: "utf8" });
  fs.rmdirSync(path.join(home, ".config/switchboard/leak-check.txt"));
  assert.equal(unreadable.status, 2, "a word list that can't be read stops it");
  const bad = spawnSync(SCRIPT, ["nope..HEAD"], { cwd: repo, env, encoding: "utf8" });
  assert.equal(bad.status, 2);
  assert.match(bad.stderr, /^leak-check: /);
});

test.after(() => fs.rmSync(tmp, { recursive: true, force: true }));
