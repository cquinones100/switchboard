#!/usr/bin/env node
// Runs a browser test against a deployed site in the system Chrome and records
// it as a video.
//
//   node record.js <steps.js> --url <url> --out <dir>   record the test
//   node record.js --login --url <url>                   sign in, by hand, once
//   --profile <name>   with either: the profile named so, in place of the URL's
//                      host, for sites whose host changes, such as a PR's
//                      preview (preview-test names one per repository)
//
// <steps.js> exports one async function, given { page, step, expect }:
//
//   module.exports = async ({ page, step }) => {
//     await step("Open the orders page");
//     await page.getByRole("link", { name: "Orders" }).click();
//   };
//
// step(text) shows text as a caption along the bottom of the page, so the
// video says what each part shows, and logs it. expect is Playwright's.
//
// Each site keeps its own Chrome profile in
// ~/.cache/deploy-test/profiles/<host>, so a site that needs signing in
// is signed into once, with --login, which opens a visible window at the URL
// and waits for you to close it; tests after that run headless with that
// session. The profile is never in a repository.
//
// Writes <dir>/test.mp4 (or test.webm without ffmpeg), <dir>/end.png and
// <dir>/log.txt (each step, console errors, and responses of 500 and up), and
// prints { passed, video, screenshot, log, error } as JSON. A step that
// throws still keeps the video up to that point.

const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { chromium } = require("playwright");
const { expect } = require("playwright/test");

const args = process.argv.slice(2);
const arg = (name) => (args.includes(`--${name}`) ? args[args.indexOf(`--${name}`) + 1] : undefined);
const url = arg("url");
const login = args.includes("--login");
const stepsFile = login ? undefined : args[0];
const out = arg("out");
if (!url || (!login && (!stepsFile || !out))) {
  console.error("usage: node record.js <steps.js> --url <url> --out <dir>\n       node record.js --login --url <url>");
  process.exit(2);
}

const size = { width: 1280, height: 800 };
// A name of letters, digits, dots, dashes and underscores only, so it stays
// a folder in the profiles directory.
const named = arg("profile");
if (named !== undefined && (!/^[\w.-]+$/.test(named) || /^\.+$/.test(named))) {
  console.error("--profile takes letters, digits, dots, dashes and underscores");
  process.exit(2);
}
const profile = path.join(os.homedir(), ".cache/deploy-test/profiles", named ?? new URL(url).host);
// Playwright runs Chrome with a stand-in keychain, so its cookies are as good
// as unencrypted: signed-in sessions, readable by no one else.
fs.mkdirSync(profile, { recursive: true, mode: 0o700 });
fs.chmodSync(path.dirname(profile), 0o700);
fs.chmodSync(profile, 0o700);

const signIn = async () => {
  const context = await chromium.launchPersistentContext(profile, { channel: "chrome", headless: false, viewport: size });
  const page = context.pages()[0] ?? (await context.newPage());
  await page.goto(url);
  await new Promise((resolve) => context.on("close", resolve));
  console.log(JSON.stringify({ profile }));
};

const record = async () => {
  fs.mkdirSync(out, { recursive: true });
  const raw = fs.mkdtempSync(path.join(out, "raw-"));
  const log = [];
  const context = await chromium.launchPersistentContext(profile, {
    channel: "chrome",
    headless: true,
    viewport: size,
    recordVideo: { dir: raw, size },
    slowMo: 300,
  });
  const page = context.pages()[0] ?? (await context.newPage());
  page.on("console", (message) => message.type() === "error" && log.push(`console error: ${message.text()}`));
  page.on("pageerror", (error) => log.push(`page error: ${error.message}`));
  page.on("response", (response) => response.status() >= 500 && log.push(`${response.status()} ${response.url()}`));

  // The caption is put back after each navigation, which would drop it.
  let caption = "";
  const showCaption = () =>
    page
      .evaluate((text) => {
        const id = "__deploy_test_caption";
        const box = document.getElementById(id) ?? document.body.appendChild(Object.assign(document.createElement("div"), { id }));
        Object.assign(box.style, {
          position: "fixed", left: "0", right: "0", bottom: "0", zIndex: "2147483647", pointerEvents: "none",
          padding: "10px 16px", font: "600 18px -apple-system, system-ui, sans-serif",
          color: "#fff", background: "rgba(20, 20, 30, 0.82)",
        });
        box.textContent = text;
      }, caption)
      .catch(() => {});
  page.on("load", () => caption && showCaption());
  const step = async (text) => {
    caption = text;
    log.push(`step: ${text}`);
    await showCaption();
    await page.waitForTimeout(1200);
  };

  const result = { passed: true, error: null };
  try {
    await page.goto(url);
    await require(path.resolve(stepsFile))({ page, step, expect });
    await page.waitForTimeout(1500);
  } catch (error) {
    result.passed = false;
    result.error = error.message.split("\n")[0];
    log.push(`failed: ${error.message}`);
  }
  const screenshot = path.join(out, "end.png");
  await page.screenshot({ path: screenshot }).catch(() => {});
  const recording = page.video();
  await context.close();

  const webm = path.join(out, "test.webm");
  fs.renameSync(await recording.path(), webm);
  fs.rmSync(raw, { recursive: true, force: true });
  // QuickTime and Slack play mp4, not webm.
  let video = webm;
  try {
    const mp4 = path.join(out, "test.mp4");
    execFileSync("ffmpeg", ["-y", "-loglevel", "error", "-i", webm, "-c:v", "libx264", "-pix_fmt", "yuv420p", "-movflags", "+faststart", mp4]);
    fs.rmSync(webm);
    video = mp4;
  } catch {}
  const logFile = path.join(out, "log.txt");
  fs.writeFileSync(logFile, `${log.join("\n")}\n`);
  console.log(JSON.stringify({ ...result, video, screenshot, log: logFile }, null, 2));
};

(login ? signIn() : record()).catch((error) => {
  console.error(error.message);
  process.exit(1);
});
