// Tests for claude/skills/walkthrough/scripts/record.js, the recorder the
// walkthrough, deploy-test and preview-test skills share. Run with:
// node --test test/*.test.js
//
// It records a walk through two pages this test serves on 127.0.0.1, in the
// system Chrome, with a temporary HOME, so the Chrome profile it makes is
// thrown away. It needs the skill's packages (npm ci in its scripts) and
// Chrome, and is skipped without them.

const { test, after } = require("node:test");
const assert = require("node:assert/strict");
const { spawn } = require("node:child_process");
const fs = require("node:fs");
const http = require("node:http");
const os = require("node:os");
const path = require("node:path");

const scripts = path.join(__dirname, "../claude/skills/walkthrough/scripts");
const chrome = "/Applications/Google Chrome.app";
// Playwright keeps its ffmpeg under the real HOME, which the run below doesn't have.
const browsers = process.env.PLAYWRIGHT_BROWSERS_PATH ?? path.join(os.homedir(), "Library/Caches/ms-playwright");
// The MP4 and the contact sheet are the system ffmpeg's.
const ffmpeg = require("node:child_process").spawnSync("ffmpeg", ["-version"]).status === 0;
// And the ffmpeg revision this Playwright records video with, in its cache.
const recorder = (() => {
  try {
    const { revision } = require(path.join(scripts, "node_modules/playwright-core/browsers.json")).browsers.find((b) => b.name === "ffmpeg");
    return fs.existsSync(path.join(browsers, `ffmpeg-${revision}`));
  } catch {
    return false;
  }
})();
const ready = fs.existsSync(path.join(scripts, "node_modules/playwright")) && fs.existsSync(chrome) && recorder && ffmpeg;
const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "walkthrough-")));
after(() => fs.rmSync(tmp, { recursive: true, force: true }));

test("record.js draws the caption, highlight, cursor and path bar into every page, and demo pace walks on past a failed expect", { skip: !ready && "needs the walkthrough skill's packages, Chrome and ffmpeg", timeout: 180_000 }, async () => {
  const pages = {
    // Home links to the orders on another origin (the same server as localhost).
    "/": "<!doctype html><title>Home</title><h1>Home</h1><a id='orders'>Orders</a><script>document.getElementById('orders').href = 'http://localhost:' + location.port + '/orders?tab=open'</script>",
    "/orders": "<!doctype html><title>Orders</title><h1>Orders</h1><table id='t'><tr><td>Order 1</td></tr></table><button onclick=\"history.pushState({}, '', '/orders?tab=closed'); document.querySelector('h2').textContent = 'Closed'\">Closed orders</button><h2></h2><dialog id='d'><button>Inside</button></dialog>",
  };
  const server = http.createServer((req, res) => {
    const body = pages[new URL(req.url, "http://x").pathname];
    res.writeHead(body ? 200 : 404, { "content-type": "text/html" });
    res.end(body ?? "");
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  after(() => server.close());
  const site = `http://127.0.0.1:${server.address().port}/`;

  // The steps read what the recorder drew, so the test sees it without the video.
  const seen = path.join(tmp, "seen.json");
  const steps = path.join(tmp, "steps.js");
  fs.writeFileSync(
    steps,
    `const fs = require("fs");
module.exports = async ({ page, step, highlight, expect }) => {
  const drawn = () => page.evaluate(() => window.__walkthroughDrawn());
  const log = [];
  await step("Open Orders from home");
  await page.getByRole("link", { name: "Orders" }).click();
  await page.waitForURL(/orders/);
  log.push(await drawn());
  await step("The orders table");
  await highlight(page.locator("#t"));
  log.push(await drawn());
  await page.getByRole("button", { name: "Closed orders" }).click();
  log.push(await drawn());
  // A modal dialog sits in the top layer: the overlay stays above it.
  await page.evaluate(() => document.getElementById("d").showModal());
  log.push(await page.evaluate(() => document.elementsFromPoint(640, 790).some((el) => el.id === "d") && document.querySelector("[popover]")?.matches(":popover-open")));
  await page.evaluate(() => document.getElementById("d").close());
  // The path bar's and caption's text is never found in place of the page's.
  await expect(page.getByText("/orders?tab=closed")).toHaveCount(0);
  await expect(page.getByText("The orders table")).toHaveCount(0);
  await expect(page.getByText("Not on the page")).toBeVisible({ timeout: 500 });
  await expect(page.locator("#t")).not.toBeVisible({ timeout: 500 });
  await expect.poll(() => page.title()).toBe("Orders");
  await expect(1).toEqual(expect.any(Number));
  await expect.configure({ timeout: 300 })(page.getByText("Also not there")).toBeVisible();
  await step("After the failed expect");
  log.push(await drawn());
  fs.writeFileSync(${JSON.stringify(seen)}, JSON.stringify(log));
};
`,
  );
  const out = path.join(tmp, "out");
  const run = spawn("node", [path.join(scripts, "record.js"), steps, "--url", site, "--out", out, "--pace", "demo", "--name", "Orders: open & closed tabs"], {
    env: { ...process.env, HOME: tmp, TMUX_PANE: "", PLAYWRIGHT_BROWSERS_PATH: browsers },
  });
  let stdout = "";
  let stderr = "";
  run.stdout.on("data", (d) => (stdout += d));
  run.stderr.on("data", (d) => (stderr += d));
  const code = await new Promise((resolve) => run.on("close", resolve));
  assert.equal(code, 0, stderr);
  const result = JSON.parse(stdout);

  const [afterNavigation, highlighted, clientSide, overDialog, afterFailure] = JSON.parse(fs.readFileSync(seen, "utf8"));
  assert.equal(overDialog, true, "the overlay is in the top layer while a modal dialog is open");
  assert.equal(afterNavigation.caption, "Open Orders from home", "the caption kept across a navigation to another origin");
  assert.equal(afterNavigation.path, "/orders?tab=open", "the path bar, query included");
  assert.equal(afterNavigation.cursor, true, "a cursor where it clicked, drawn as SVG");
  assert.equal(highlighted.highlighted, "block", "the highlight drawn");
  assert.equal(clientSide.path, "/orders?tab=closed", "the path bar after a client-side navigation");
  assert.equal(afterFailure.highlighted, "none", "the highlight cleared at the next step");
  assert.equal(afterFailure.caption, "After the failed expect", "demo pace walks on past failed expects, a .not one too");

  assert.equal(result.passed, false, "but the failed expect is reported");
  assert.equal(fs.readFileSync(result.log, "utf8").match(/expect failed:/g).length, 3, fs.readFileSync(result.log, "utf8"));
  assert.ok(fs.existsSync(result.video), "the video");
  assert.ok(result.contact && fs.existsSync(result.contact), "the contact sheet");
  assert.equal(path.basename(result.video, path.extname(result.video)), "Orders-open-closed-tabs", "named by --name, made a file name");
});
