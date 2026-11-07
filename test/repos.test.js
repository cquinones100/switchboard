// Tests for lib/repos.js and bin/landing: how each repository lands its work.
// Run with: node --test test/*.test.js
//
// Each test works in a temporary repository, with repos.json and the cache in
// a temporary directory, and GitHub and Claude stubbed, so nothing reaches
// either or reads your own repos.json.

const { test, beforeEach, after } = require("node:test");
const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "repos-test-")));
// HOME too, so the requests the stubs count stay out of yours.
process.env.HOME = tmp;
process.env.SWITCHBOARD_REPOS = path.join(tmp, "repos.json");
process.env.TMUX_SIDEBAR_STATE_DIR = path.join(tmp, "state");
process.env.SWITCHBOARD_SETTINGS = path.join(tmp, "settings.json");
const repos = require("../lib/repos");

const env = { ...process.env, GIT_CONFIG_GLOBAL: "/dev/null" };
const repoAt = (name, origin) => {
  const dir = path.join(tmp, name);
  execFileSync("git", ["init", "-q", "-b", "main", dir], { env });
  if (origin) execFileSync("git", ["-C", dir, "remote", "add", "origin", origin], { env });
  return dir;
};
const direct = repoAt("direct", "git@github.com:acme/direct.git");
const guarded = repoAt("guarded", "https://github.com/acme/guarded.git");

after(() => fs.rmSync(tmp, { recursive: true, force: true }));

const never = () => {
  throw new Error("not to be asked");
};

beforeEach(() => {
  fs.rmSync(process.env.SWITCHBOARD_SETTINGS, { force: true });
  fs.rmSync(process.env.SWITCHBOARD_REPOS, { force: true });
  fs.rmSync(process.env.TMUX_SIDEBAR_STATE_DIR, { recursive: true, force: true });
  fs.rmSync(path.join(direct, "CLAUDE.md"), { force: true });
});

test("a repository's entry in repos.json decides, with its steps, asking neither GitHub nor Claude", async () => {
  fs.writeFileSync(process.env.SWITCHBOARD_REPOS, JSON.stringify({ "acme/direct": { landing: "direct", test: "npm test", squash: false } }));
  const answer = await repos.landingFor(direct, { ask: { github: never, claude: never } });
  assert.equal(answer.landing, "direct");
  assert.equal(answer.source, "repos.json");
  assert.deepEqual(answer.steps, { test: "npm test", squash: false, review: "review-loop", beforePush: "" }, "what the entry leaves out takes the defaults");
  fs.writeFileSync(process.env.SWITCHBOARD_REPOS, JSON.stringify({ "acme/direct": { landing: "direct" } }));
  assert.equal((await repos.landingFor(direct, { ask: { github: never, claude: never } })).steps.test, null, "a test command not set is null, for the CLAUDE.md's tests, not none");
  fs.writeFileSync(process.env.SWITCHBOARD_REPOS, "{ broken");
  await assert.rejects(repos.landingFor(direct, { ask: { github: never, claude: never } }), /can't be read/, "a broken repos.json stops it");
  fs.writeFileSync(process.env.SWITCHBOARD_REPOS, JSON.stringify({ "acme/direct": { landing: "direct", test: "npm test", squash: false } }));
  const printed = execFileSync(process.execPath, [path.join(__dirname, "../bin/landing"), direct], { env: process.env, encoding: "utf8" }).split("\n");
  assert.equal(printed[0], "direct");
  assert.equal(JSON.parse(printed[1]).test, "npm test");
  assert.equal(JSON.parse(execFileSync(process.execPath, [path.join(__dirname, "../bin/landing"), "--json", direct], { env: process.env, encoding: "utf8" })).landing, "direct");
});

test("with no entry, a CLAUDE.md that commits straight to the default branch makes it direct, kept a day, and GitHub requiring a PR wins over it", async () => {
  fs.writeFileSync(path.join(direct, "CLAUDE.md"), "Commit straight to main: there is no PR flow here.\n");
  let read;
  const answer = await repos.landingFor(direct, { ask: { github: async () => ({ branch: "main", requiresPr: false }), claude: async (text) => ((read = text), "direct") } });
  assert.equal(answer.landing, "direct");
  assert.equal(answer.source, "claude-md");
  assert.match(answer.why, /its CLAUDE.md commits straight to main/);
  assert.match(read, /no PR flow/, "Claude is given the CLAUDE.md");
  const again = await repos.landingFor(direct, { ask: { github: never, claude: never } });
  assert.equal(again.landing, "direct");
  assert.equal(again.source, "cached", "a second ask within the day is answered from the cache");

  const pr = await repos.landingFor(guarded, { ask: { github: async () => ({ branch: "main", requiresPr: true, why: "main has a merge queue" }), claude: never } });
  assert.equal(pr.landing, "pr");
  assert.equal(pr.source, "github");
  assert.equal(pr.why, "main has a merge queue");
});

test("off the network, the last answer stands, however old, and with none it is a pull request", async () => {
  const offline = { github: async () => { throw new Error("no network"); }, claude: never };
  const none = await repos.landingFor(guarded, { ask: offline });
  assert.equal(none.landing, "pr");
  assert.equal(none.source, "offline");

  fs.writeFileSync(path.join(direct, "CLAUDE.md"), "Commit straight to main.\n");
  await repos.landingFor(direct, { ask: { github: async () => ({ branch: "main", requiresPr: false }), claude: async () => "direct" } });
  const later = await repos.landingFor(direct, { ask: offline, now: Date.now() + 2 * repos.DAY_MS });
  assert.equal(later.landing, "direct");
  assert.equal(later.source, "offline");
});

test("setRepo keeps the rest of an entry, automatic removes only its method, and a file that can't be read is never written over", () => {
  repos.setRepo("acme/direct", "landing", "direct");
  repos.setRepo("acme/direct", "test", "make check");
  assert.deepEqual(repos.readRepos(), { "acme/direct": { landing: "direct", test: "make check" } });
  repos.setRepo("acme/direct", "landing", "automatic");
  assert.deepEqual(repos.readRepos(), { "acme/direct": { test: "make check" } }, "the default decides again, the steps kept");
  repos.setRepo("acme/direct", "test", "");
  assert.throws(() => repos.setRepo("acme/direct", "review", "maybe"), /not a value/);
  fs.writeFileSync(process.env.SWITCHBOARD_REPOS, "{ not json");
  assert.throws(() => repos.setRepo("acme/direct", "landing", "pr"), /can't be read/);
  assert.equal(fs.readFileSync(process.env.SWITCHBOARD_REPOS, "utf8"), "{ not json");
});

test("GitHub's rulesets that can't be read are a failure to find out, but a repository without them (404) has none", async (t) => {
  const bin = path.join(tmp, "gh-bin");
  fs.mkdirSync(bin, { recursive: true });
  const stub = (rest) => fs.writeFileSync(path.join(bin, "gh"), `#!/bin/sh\ncase "$2" in graphql) echo '{"data":{"repository":{"mergeQueue":null,"defaultBranchRef":{"name":"main","refUpdateRule":null}}}}' ;; *) ${rest} ;; esac\n`, { mode: 0o755 });
  const path0 = process.env.PATH;
  process.env.PATH = `${bin}:${path0}`;
  t.after(() => (process.env.PATH = path0));
  stub(`echo 'gh: Not Found (HTTP 404)' >&2; exit 1`);
  assert.deepEqual(await repos.askGithub("acme/direct"), { branch: "main", requiresPr: false });
  stub(`echo 'gh: Server Error (HTTP 500)' >&2; exit 1`);
  await assert.rejects(repos.askGithub("acme/direct"), /500/);
  stub(`echo '[{"type":"pull_request"}]'`);
  assert.equal((await repos.askGithub("acme/direct")).requiresPr, true);
  fs.writeFileSync(path.join(bin, "gh"), `#!/bin/sh\necho '{"data":{"repository":{"mergeQueue":null,"defaultBranchRef":{"name":"main","refUpdateRule":{"viewerCanPush":true,"requiredApprovingReviewCount":0}}}}}'\n`, { mode: 0o755 });
  assert.equal((await repos.askGithub("acme/direct")).requiresPr, true, "branch protection requiring a PR with no approvals still requires one");
});

test("a CLAUDE.md that links out of its repository is never read or sent to Claude", async () => {
  fs.writeFileSync(path.join(tmp, "secret"), "a key\n");
  fs.symlinkSync(path.join(tmp, "secret"), path.join(direct, "CLAUDE.md"));
  const answer = await repos.landingFor(direct, { ask: { github: async () => ({ branch: "main", requiresPr: false }), claude: never } });
  assert.equal(answer.landing, "pr");
});

test("with the github setting off, GitHub isn't asked: the last answer stands, else a pull request", async () => {
  fs.writeFileSync(process.env.SWITCHBOARD_SETTINGS, JSON.stringify({ github: false }));
  const answer = await repos.landingFor(direct, { ask: { github: never, claude: never } });
  assert.equal(answer.landing, "pr");
  assert.match(answer.why, /github setting is off/);
});
