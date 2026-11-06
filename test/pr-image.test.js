// The PR and ticket image view's caches (tmux/pr-image.js): what the page
// loads from the web, kept by its URL without the query, and the pages drawn,
// kept by the text and the pane's size. Run with: node --test test/*.test.js
//
// Chrome is never started: the asset cache is driven with stand-ins for
// puppeteer's requests and responses, and the open that finds its pages in
// the cache runs with a pnpm that fails the test if the script looks for
// puppeteer at all.

const { test, after } = require("node:test");
const assert = require("node:assert/strict");
const { execFileSync, spawnSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const { sameText, GUESSED_CELL, viewFor, assetCache, assetKey, cachedPages, savePages, prune, assetDir, pagesDir } = require("../tmux/pr-image");

const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "pr-image-")));
after(() => fs.rmSync(tmp, { recursive: true, force: true }));
let roots = 0;
const newRoot = () => fs.mkdtempSync(path.join(tmp, `root-${++roots}-`));

// A puppeteer request and its response, as the cache sees them.
const request = (url, type = "image") => {
  const r = {
    url: () => url,
    resourceType: () => type,
    redirectChain: () => [],
    answered: undefined,
    continued: false,
    respond: (answer) => (r.answered = answer),
    continue: () => (r.continued = true),
  };
  return r;
};
const response = (req, status, body, type = "image/png") => ({
  url: () => req.url(),
  status: () => status,
  request: () => req,
  headers: () => ({ "content-type": type }),
  buffer: async () => Buffer.from(body),
});

test("a picture is fetched once, and read from the cache with a new token in its link, counting nothing", async () => {
  const root = newRoot();
  const counted = [];
  const cache = assetCache(root, (to) => counted.push(to.hostname));

  const first = request("https://uploads.linear.app/u/1.png?token=a");
  cache.onRequest(first);
  assert.equal(first.continued, true);
  await cache.onResponse(response(first, 200, "picture"));
  assert.deepEqual(counted, ["uploads.linear.app"]);

  const second = request("https://uploads.linear.app/u/1.png?token=b");
  cache.onRequest(second);
  assert.equal(second.continued, false);
  assert.equal(second.answered.status, 200);
  assert.equal(second.answered.contentType, "image/png");
  assert.equal(second.answered.body.toString(), "picture");
  assert.deepEqual(counted, ["uploads.linear.app"]);
});

test("a query that names the picture is kept apart, outside the signed hosts", () => {
  assert.notEqual(assetKey("https://images.example.com/image?id=1"), assetKey("https://images.example.com/image?id=2"));
  assert.equal(assetKey("https://private-user-images.githubusercontent.com/1/a.png?jwt=x"), assetKey("https://private-user-images.githubusercontent.com/1/a.png?jwt=y"));
});

test("text read again with only its pictures' signatures new finds the same drawn pages", () => {
  const root = newRoot();
  const view = viewFor(GUESSED_CELL, 80, 24);
  const read = (sig) => ({ title: "T", number: "#1", meta: "", html: `<img src="https://private-user-images.githubusercontent.com/1/a.png?jwt=${sig}&amp;x=1">`, markdown: [`![](https://uploads.linear.app/a/b.png?signature=${sig})`] });
  assert.equal(sameText(read("one")), sameText(read("two")));
  savePages(root, read("one"), view, [{ png: Buffer.from("page"), rows: 10, from: 0 }]);
  assert.equal(cachedPages(root, read("two"), view)[0].png.toString(), "page");
  assert.notEqual(sameText({ html: '<img src="https://images.example.com/i?id=1">' }), sameText({ html: '<img src="https://images.example.com/i?id=2">' }));
});

test("a failed download isn't cached", async () => {
  const root = newRoot();
  const cache = assetCache(root, () => {});
  const missing = request("https://uploads.linear.app/u/2.png?token=a");
  cache.onRequest(missing);
  await cache.onResponse(response(missing, 404, "not found"));
  assert.equal(fs.existsSync(path.join(assetDir(root), assetKey(missing.url()))), false);

  const again = request("https://uploads.linear.app/u/2.png?token=b");
  cache.onRequest(again);
  assert.equal(again.continued, true);
});

test("a redirected picture is kept under the link the page asked for", async () => {
  const root = newRoot();
  const cache = assetCache(root, () => {});
  const asked = request("https://github.com/user-attachments/assets/abc");
  const landed = { ...request("https://objects.example.com/abc?sig=1"), redirectChain: () => [asked] };
  await cache.onResponse(response(landed, 200, "picture"));

  const again = request("https://github.com/user-attachments/assets/abc");
  cache.onRequest(again);
  assert.equal(again.answered.body.toString(), "picture");
});

test("the drawn pages are found again for the same text and size, and drawn again for another width", () => {
  const root = newRoot();
  const parts = { title: "T", number: "#1", meta: "", html: "<p>hi</p>" };
  const view = viewFor(GUESSED_CELL, 80, 24);
  savePages(root, parts, view, [{ png: Buffer.from("one"), rows: 10, from: 0 }, { png: Buffer.from("two"), rows: 8, from: 0.5 }]);

  const shots = cachedPages(root, parts, view);
  assert.deepEqual(shots.map((s) => [s.png.toString(), s.rows, s.from]), [["one", 10, 0], ["two", 8, 0.5]]);
  assert.ok(shots.every((s) => fs.existsSync(s.file)));

  assert.equal(cachedPages(root, parts, viewFor(GUESSED_CELL, 100, 24)), undefined);
  assert.equal(cachedPages(root, { ...parts, html: "<p>changed</p>" }, view), undefined);
});

test("pruning removes the least recently used first once over the limit, and page sets over 30 days old", () => {
  const root = newRoot();
  const now = Date.now();
  const at = (file, msAgo) => fs.utimesSync(file, new Date(now - msAgo), new Date(now - msAgo));
  fs.mkdirSync(assetDir(root), { recursive: true });
  const asset = (name, msAgo) => {
    const file = path.join(assetDir(root), name);
    fs.writeFileSync(file, Buffer.alloc(100));
    fs.writeFileSync(`${file}.meta`, "{}");
    at(file, msAgo);
  };
  asset("old", 3000);
  asset("middle", 2000);
  asset("new", 1000);
  const pageSet = (name, msAgo) => {
    const dir = path.join(pagesDir(root), name);
    fs.mkdirSync(dir, { recursive: true });
    fs.writeFileSync(path.join(dir, "page-1.png"), Buffer.alloc(10));
    at(dir, msAgo);
  };
  pageSet("recent", 500);
  pageSet("stale", 31 * 24 * 60 * 60 * 1000);

  prune(root, 250, now);
  assert.deepEqual(fs.readdirSync(assetDir(root)).sort(), ["middle", "middle.meta", "new", "new.meta"]);
  assert.deepEqual(fs.readdirSync(pagesDir(root)), ["recent"]);
});

test("pruning keeps the page set on screen, however old, and counts it toward the limit", () => {
  const root = newRoot();
  const now = Date.now();
  const dir = path.join(pagesDir(root), "showing");
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, "page-1.png"), Buffer.alloc(200));
  fs.utimesSync(dir, new Date(now - 10_000), new Date(now - 10_000));
  fs.mkdirSync(assetDir(root), { recursive: true });
  fs.writeFileSync(path.join(assetDir(root), "picture"), Buffer.alloc(100));

  prune(root, 250, now, "showing");
  assert.deepEqual(fs.readdirSync(pagesDir(root)), ["showing"]);
  assert.deepEqual(fs.readdirSync(assetDir(root)), []);
});

test("with its text and pages cached, a PR opens without looking for Chrome", () => {
  const root = path.join(tmp, "open");
  const xdg = path.join(root, "cache");
  const bin = path.join(root, "bin");
  const repo = path.join(root, "repo");
  const dump = path.join(root, "dump");
  const looked = path.join(root, "looked-for-puppeteer");
  [bin, dump].forEach((dir) => fs.mkdirSync(dir, { recursive: true }));
  execFileSync("git", ["init", "-q", "-b", "feature/x", repo]);
  fs.writeFileSync(path.join(bin, "pnpm"), `#!/bin/sh\ntouch '${looked}'\nexit 1\n`, { mode: 0o755 });
  fs.writeFileSync(path.join(bin, "gh"), "#!/bin/sh\nexit 1\n", { mode: 0o755 });

  const parts = { title: "T", number: "#7", meta: "", html: "<p>hi</p>" };
  const gitDir = execFileSync("git", ["rev-parse", "--absolute-git-dir"], { cwd: repo, encoding: "utf8" }).trim();
  const textFile = path.join(xdg, "switchboard", "pr-image", `${`${gitDir}:feature/x`.replace(/\//g, "%")}.json`);
  fs.mkdirSync(path.dirname(textFile), { recursive: true });
  fs.writeFileSync(textFile, JSON.stringify(parts));
  // The script, with no terminal to ask, takes the guessed cell and an 80 by 24 pane.
  savePages(path.join(xdg, "switchboard"), parts, viewFor(GUESSED_CELL, undefined, undefined), [{ png: Buffer.from("page one"), rows: 10, from: 0 }]);

  const env = { ...process.env, HOME: root, XDG_CACHE_HOME: xdg, PATH: `${bin}:/usr/bin:/bin`, PR_IMAGE_DUMP: dump, SWITCHBOARD_SETTINGS: path.join(root, "settings.json") };
  delete env.TMUX;
  delete env.TMUX_PANE;
  const run = spawnSync(process.execPath, [path.join(__dirname, "..", "tmux", "pr-image.js"), repo], { env, stdio: ["ignore", "pipe", "pipe"], encoding: "utf8", timeout: 20_000 });
  assert.equal(run.status, 0, run.stdout + run.stderr);
  assert.equal(fs.existsSync(looked), false);
  assert.equal(fs.readFileSync(path.join(dump, "page-1.png"), "utf8"), "page one");
});
