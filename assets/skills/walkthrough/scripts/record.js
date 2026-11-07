#!/usr/bin/env node
// Records a walk through a site in the system Chrome as a captioned video: the
// walkthrough skill's recorder, which deploy-test and preview-test record their
// browser tests with too.
//
//   node record.js <steps.js> --url <url> --out <dir>   record it
//   node record.js --login --url <url>                   sign in, by hand, once
//   --profile <name>   with either: the profile named so, in place of the URL's
//                      host, for sites whose host changes, such as a PR's
//                      preview (preview-test names one per repository)
//   --pace demo|test   demo (walkthrough) holds each step longer, for someone
//                      watching, and carries on past an expect that fails,
//                      noting it; test (the default) keeps a test's pace and
//                      stops at the first failure
//
// <steps.js> exports one async function, given { page, step, highlight, expect }:
//
//   module.exports = async ({ page, step, highlight }) => {
//     await step("Open the orders page");
//     await page.getByRole("link", { name: "Orders" }).click();
//     await highlight(page.getByRole("table"));
//   };
//
// step(text) shows text as a caption in a strip under the page and logs it.
// highlight(locator) outlines what is being shown, until the next step. expect
// is Playwright's. Drawn into every page, across navigations, by one init
// script, so no steps.js has to remember them: a cursor that moves to what is
// clicked (headless video has none) and a ripple where it clicks. Above the
// page, an address bar as a browser draws one, with the page's whole address,
// kept up to date on every navigation, client-side ones (pushState) included.
// The address bar and the caption take space of their own, never covering
// the page: they are laid round the recording afterwards, so the video is
// taller than the page by both.
//
// Each site keeps its own Chrome profile in ~/.cache/deploy-test/profiles/<host>
// (the path from before this was the walkthrough skill's, kept so no one signs
// in again), so a site that needs signing in is signed into once, with --login,
// which opens a visible window at the URL and waits for you to close it; runs
// after that are headless with that session. The profile is never in a
// repository.
//
// Writes <dir>/<name>.mp4, named by --name, a few
// words saying what the video shows that the skill writes, so the video says
// what it is when it opens; without it, after the Switchboard tab it runs in
// (the tab's name, as the tab bar shows it), and outside one walkthrough.mp4. Also
// <dir>/contact.png, a frame every two seconds in a grid, to check the video
// at a glance; <dir>/end.png; and <dir>/log.txt (each step, console errors,
// responses of 500 and up, and in demo pace each expect that failed). Prints
// { passed, video, contact, screenshot, log, error, timeline } as JSON, the
// timeline being each address and caption with the second it began. A step that
// throws still keeps the video up to that point.

const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { chromium } = require("playwright");
const { expect: playwrightExpect } = require("playwright/test");

const args = process.argv.slice(2);
const arg = (name) => (args.includes(`--${name}`) ? args[args.indexOf(`--${name}`) + 1] : undefined);
const url = arg("url");
const login = args.includes("--login");
const stepsFile = login ? undefined : args[0];
const out = arg("out");
const pace = arg("pace") ?? "test";
if (!url || (!login && (!stepsFile || !out)) || !["demo", "test"].includes(pace)) {
  console.error("usage: node record.js <steps.js> --url <url> --out <dir> [--pace demo|test] [--profile <name>] [--name <title>]\n       node record.js --login --url <url> [--profile <name>]");
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

// The video's name: --name, else the Switchboard tab this runs in, as the tab
// bar names it (@tab_label, asked of the server with `switchboard show`, as it
// was of tmux), made a file name the Slack relay finds in a message
// (files.js's MEDIA_PATH takes ASCII letters, digits, . _ - and no spaces):
// anything else dropped, each run of spaces a dash, and cut to 80
// characters; "walkthrough" outside Switchboard or for a tab with no name.
const fileName = (text) => text.replace(/[^A-Za-z0-9 ._-]/g, "").trim().replace(/\s+/g, "-").replace(/-{2,}/g, "-").slice(0, 80).replace(/^[.-]+|[.-]+$/g, "");
const videoName = () => {
  if (arg("name") && fileName(arg("name"))) return fileName(arg("name"));
  if (!process.env.SWITCHBOARD_PANE) return "walkthrough";
  try {
    const label = execFileSync("switchboard", ["show", "-t", process.env.SWITCHBOARD_PANE, "@tab_label"], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] });
    return fileName(label) || "walkthrough";
  } catch {
    return "walkthrough";
  }
};

// Runs in every page before its own scripts, so it is there after every
// navigation: the outline round what is highlighted, and the cursor and its
// ripple, which point at the page itself. (The caption and the address bar
// take space of their own, above and below the page, added to the video
// afterwards: frame.) What it shows comes from the page's session storage,
// which outlives a navigation within the site, so the next page draws the
// same cursor. All of it ignores the pointer, so
// none of it is ever clicked in place of the page, and all of it is in a closed
// shadow root, so no locator in a steps.js finds its text in place of the
// page's own. Only the top frame draws it.
const overlay = () => {
  if (window !== window.top) return;
  const KEY = "__walkthrough";
  // The host is a popover, shown, so it is in the browser's top layer, where
  // a modal dialog is too; opening one puts it back on top (raise).
  let root;
  const raise = () => {
    const host = root?.host;
    if (!host?.isConnected || !host.showPopover) return;
    try {
      host.hidePopover();
      host.showPopover();
    } catch {}
  };
  const layer = () => {
    if (!root || !root.host.isConnected) {
      const host = document.createElement("div");
      host.setAttribute("popover", "manual");
      Object.assign(host.style, { position: "fixed", inset: "0", width: "100vw", height: "100vh", margin: "0", padding: "0", border: "0", background: "transparent", overflow: "visible", pointerEvents: "none" });
      document.documentElement.appendChild(host);
      root = host.attachShadow({ mode: "closed" });
      raise();
    }
    return root;
  };
  const showModal = HTMLDialogElement.prototype.showModal;
  HTMLDialogElement.prototype.showModal = function (...rest) {
    const result = showModal.apply(this, rest);
    raise();
    return result;
  };
  const read = () => {
    try {
      return JSON.parse(sessionStorage.getItem(KEY) || "{}");
    } catch {
      return {};
    }
  };
  const box = (id, style) => {
    let el = layer().getElementById(id);
    if (!el) {
      el = document.createElement("div");
      el.id = id;
      layer().appendChild(el);
    }
    Object.assign(el.style, { position: "fixed", zIndex: "2147483647", pointerEvents: "none" }, style);
    return el;
  };
  const draw = () => {
    if (!document.documentElement) return;
    const { cursor: at } = read();
    if (at) place(at[0], at[1]);
  };
  // The outline follows what it is round as the page scrolls or moves.
  const outline = () => {
    const ring = box("__walkthrough_highlight", { border: "3px solid #f2cc60", borderRadius: "6px", boxShadow: "0 0 0 4px rgba(242, 204, 96, 0.35)", transition: "all 0.2s" });
    const target = window.__walkthroughTarget;
    // An element in this page, followed as it moves; or a fixed rectangle,
    // for one in a frame, where its own position means nothing here.
    const r = target?.rect ?? (target?.isConnected ? target.getBoundingClientRect() : null);
    if (!r) {
      ring.style.display = "none";
      return;
    }
    Object.assign(ring.style, { display: "block", left: `${r.left - 6}px`, top: `${r.top - 6}px`, width: `${r.width + 6}px`, height: `${r.height + 6}px` });
  };
  window.__walkthroughSet = (state) => {
    sessionStorage.setItem(KEY, JSON.stringify({ ...read(), ...state }));
    draw();
  };
  window.__walkthroughHighlight = (el, rect) => {
    window.__walkthroughTarget = rect ? { rect: { left: rect.x, top: rect.y, width: rect.width, height: rect.height } } : el;
    outline();
  };
  // What is drawn, for the tests, which can't see into the shadow root.
  window.__walkthroughDrawn = () => {
    const el = (id) => layer().getElementById(id);
    return { highlighted: el("__walkthrough_highlight")?.style.display, cursor: Boolean(el("__walkthrough_cursor")?.querySelector("svg path")) };
  };
  // The cursor, moved to where the page is clicked, with a ripple there.
  // Drawn as SVG elements, not an image, which a site's Content-Security-Policy
  // could block.
  const cursor = () => {
    const el = box("__walkthrough_cursor", { width: "18px", height: "18px", marginLeft: "-3px", marginTop: "-2px", transition: "left 0.35s ease, top 0.35s ease" });
    if (!el.firstChild) {
      const NS = "http://www.w3.org/2000/svg";
      const svg = document.createElementNS(NS, "svg");
      svg.setAttribute("width", "18");
      svg.setAttribute("height", "18");
      const arrow = document.createElementNS(NS, "path");
      arrow.setAttribute("d", "M2 1 L2 15 L6 11 L9 17 L11 16 L8 10 L14 10 Z");
      arrow.setAttribute("fill", "white");
      arrow.setAttribute("stroke", "black");
      arrow.setAttribute("stroke-width", "1.2");
      svg.appendChild(arrow);
      el.appendChild(svg);
    }
    return el;
  };
  // Where it is is kept with the rest, so the next page draws it there.
  const place = (x, y) => Object.assign(cursor().style, { left: `${x}px`, top: `${y}px` });
  window.__walkthroughMove = (x, y) => {
    place(x, y);
    sessionStorage.setItem(KEY, JSON.stringify({ ...read(), cursor: [x, y] }));
  };
  // The ripple where a click lands, drawn by the recorder as it clicks, in
  // the top frame, so a click inside a frame gets one too.
  window.__walkthroughRipple = (x, y) => {
    const ripple = box(`__walkthrough_ripple_${Date.now()}`, {
      left: `${x - 14}px`, top: `${y - 14}px`, width: "28px", height: "28px",
      borderRadius: "50%", border: "3px solid #f2cc60", transition: "transform 0.5s ease-out, opacity 0.5s ease-out",
    });
    requestAnimationFrame(() => Object.assign(ripple.style, { transform: "scale(2.2)", opacity: "0" }));
    setTimeout(() => ripple.remove(), 600);
  };
  // While something is highlighted, the outline is measured again often, so it
  // follows the element when the page's layout moves it.
  setInterval(() => window.__walkthroughTarget && outline(), 100);
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", draw);
  else draw();
};

// The address bar over the page and the caption under it, each taking space
// of its own rather than covering the page: drawn as images by Chrome, one
// for each address and each caption, and laid round the recording by ffmpeg
// (a strip padded above and below), each shown from when it began to when the
// next did. `timeline` is { urls, captions }, each [{ at: seconds, text }].
// Returns the MP4, or nothing when it couldn't be made.
const BAR = 52;
const CAPTION = 76;
const frame = async (timeline, webm, mp4, log) => {
  const dir = fs.mkdtempSync(path.join(out, "frame-"));
  let browser;
  try {
    browser = await chromium.launch({ channel: "chrome", headless: true });
    const page = await browser.newPage({ viewport: { width: size.width, height: CAPTION } });
    const draw = async (kind, text, file) => {
      await page.setViewportSize({ width: size.width, height: kind === "bar" ? BAR : CAPTION });
      await page.setContent(`<!doctype html><style>
        body { margin: 0; font: 15px -apple-system, system-ui, sans-serif; }
        .bar { height: ${BAR}px; box-sizing: border-box; padding: 0 14px; background: #dee1e6; display: flex; align-items: center; gap: 14px; color: #5f6368; }
        .nav { font-size: 18px; letter-spacing: 10px; }
        .pill { flex: 1; height: 34px; border-radius: 17px; background: #fff; display: flex; align-items: center; gap: 8px; padding: 0 14px; overflow: hidden; white-space: nowrap; }
        .address { overflow: hidden; text-overflow: ellipsis; } .host { color: #202124; } .rest { color: #5f6368; }
        .caption { height: ${CAPTION}px; box-sizing: border-box; padding: 0 24px; background: #14141e; color: #fff; display: flex; align-items: center; font: 600 21px -apple-system, system-ui, sans-serif; }
        .caption span { display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; }
      </style>
      <div class="bar" id="bar"><span class="nav">←→↻</span><div class="pill"><svg id="lock" width="12" height="14" viewBox="0 0 12 14"><rect x="1" y="6" width="10" height="7" rx="1.5" fill="#5f6368"/><path d="M3.5 6V4a2.5 2.5 0 0 1 5 0v2" stroke="#5f6368" stroke-width="1.6" fill="none"/></svg><span class="address"><span class="host" id="host"></span><span class="rest" id="rest"></span></span></div></div>
      <div class="caption" id="caption"><span id="text"></span></div>`);
      // The text set as text, never as markup: a page's address or a caption
      // is data. A caption too long for two lines is drawn smaller, down to
      // 15px; one that still doesn't fit is cut short, and said so (false).
      const fits = await page.evaluate(([which, value]) => {
        document.getElementById(which === "bar" ? "caption" : "bar").remove();
        if (which === "bar") {
          let parsed = null;
          try {
            parsed = new URL(value);
          } catch {}
          // The lock only for https, as a browser shows it.
          if (parsed?.protocol !== "https:") document.getElementById("lock").remove();
          document.getElementById("host").textContent = parsed ? parsed.host : value;
          document.getElementById("rest").textContent = parsed ? parsed.pathname + parsed.search + parsed.hash : "";
        } else {
          const span = document.getElementById("text");
          span.textContent = value;
          for (let px = 21; px >= 15; px -= 1) {
            span.style.fontSize = `${px}px`;
            if (span.scrollHeight <= span.clientHeight + 1) return true;
          }
          return false;
        }
        return true;
      }, [kind, text]);
      await page.screenshot({ path: file });
      return fits;
    };
    // Each change shown from when it came to when the next did.
    const spans = (list) => list.map((item, i) => ({ ...item, until: list[i + 1]?.at ?? 1e6 })).filter((item) => item.until > item.at);
    const layers = [];
    for (const [i, item] of spans(timeline.urls).entries()) {
      const file = path.join(dir, `bar-${i}.png`);
      await draw("bar", item.text, file);
      layers.push({ file, y: 0, ...item });
    }
    for (const [i, item] of spans(timeline.captions).entries()) {
      if (!item.text) continue;
      const file = path.join(dir, `caption-${i}.png`);
      if (!(await draw("caption", item.text, file))) log.push(`caption cut short, too long for two lines: ${item.text}`);
      layers.push({ file, y: BAR + size.height, ...item });
    }
    const filters = [`[0:v]pad=${size.width}:${size.height + BAR + CAPTION}:0:${BAR}:color=0x14141e[v0]`];
    layers.forEach((layer, i) => filters.push(`[v${i}][${i + 1}:v]overlay=0:${layer.y}:enable='between(t,${layer.at.toFixed(3)},${layer.until.toFixed(3)})'[v${i + 1}]`));
    execFileSync("ffmpeg", [
      "-y", "-loglevel", "error", "-i", webm,
      ...layers.flatMap((layer) => ["-i", layer.file]),
      "-filter_complex", filters.join(";"), "-map", `[v${layers.length}]`,
      "-c:v", "libx264", "-pix_fmt", "yuv420p", "-movflags", "+faststart", mp4,
    ]);
    return mp4;
  } catch (error) {
    return null;
  } finally {
    await browser?.close().catch(() => {});
    fs.rmSync(dir, { recursive: true, force: true });
  }
};

const record = async () => {
  // The system ffmpeg lays the address bar and captions round the video and
  // makes the MP4, so a recording without it would show neither: asked for
  // before anything is recorded.
  try {
    execFileSync("ffmpeg", ["-version"], { stdio: "ignore" });
  } catch {
    throw new Error("ffmpeg is needed to make the video, with its address bar and captions: install it (brew install ffmpeg) and run again");
  }
  fs.mkdirSync(out, { recursive: true });
  const raw = fs.mkdtempSync(path.join(out, "raw-"));
  const log = [];
  const demo = pace === "demo";
  const context = await chromium.launchPersistentContext(profile, {
    channel: "chrome",
    headless: true,
    viewport: size,
    recordVideo: { dir: raw, size },
    slowMo: demo ? 500 : 300,
  });
  // When the recording began, for the times the address bar and the caption
  // change at (frame).
  const began = Date.now();
  const timeline = { urls: [], captions: [] };
  const now = () => (Date.now() - began) / 1000;
  await context.addInitScript(overlay);
  const page = context.pages()[0] ?? (await context.newPage());
  page.on("console", (message) => message.type() === "error" && log.push(`console error: ${message.text()}`));
  page.on("pageerror", (error) => log.push(`page error: ${error.message}`));
  // Every address the page shows, a client-side navigation's (pushState) too.
  page.on("framenavigated", (navigated) => {
    if (navigated === page.mainFrame() && timeline.urls.at(-1)?.text !== navigated.url()) timeline.urls.push({ at: now(), text: navigated.url() });
  });
  page.on("response", (response) => response.status() >= 500 && log.push(`${response.status()} ${response.url()}`));
  // A page opened before the init script took (the persistent context's
  // first) gets it now.
  await page.evaluate(overlay).catch(() => {});

  // The cursor goes to what is about to be clicked, and is seen getting
  // there, before Playwright clicks it; the mousedown draws the ripple. Done
  // on the locator's own methods, which every steps.js clicks through.
  // A steps.js that sets a default timeout (on the page or its context) means
  // every action to keep it: from then on none gets the cursor's wait.
  let defaultTimed = false;
  [page, context].forEach((on) => {
    const set = on.setDefaultTimeout.bind(on);
    on.setDefaultTimeout = (ms) => {
      defaultTimed = true;
      return set(ms);
    };
  });
  const Locator = Object.getPrototypeOf(page.locator("body"));
  ["click", "dblclick", "check", "uncheck", "hover", "tap", "fill", "selectOption"].forEach((name) => {
    const original = Locator[name];
    if (typeof original !== "function") return;
    Locator[name] = async function (...callArgs) {
      // The action's options: its last argument when that is an object (fill
      // and selectOption take a value first).
      const last = callArgs[callArgs.length - 1];
      const options = last && typeof last === "object" && !Array.isArray(last) ? last : undefined;
      // Only an element already there: waiting for it here would add to the
      // action's own timeout, and an action that should time out wouldn't.
      // Scrolled into view first, as the action would, so the cursor goes
      // where the click will land, on screen: options.position (from the
      // element's corner) when given, else its middle.
      // An action given a timeout of its own is left exactly as asked: no
      // cursor, so nothing spends any of that time.
      const timed = defaultTimed || (options && typeof options.timeout === "number");
      let at = null;
      if (!timed && (await this.isVisible().catch(() => false))) {
        // Not for an action told not to scroll: the cursor then only goes to an
        // element already on screen.
        if (options?.scroll !== "none") await this.scrollIntoViewIfNeeded({ timeout: 500 }).catch(() => {});
        const box = await this.boundingBox({ timeout: 500 }).catch(() => null);
        const view = page.viewportSize();
        // Playwright measures a position from inside the element's border.
        const border = options?.position ? await this.evaluate((el) => { const css = getComputedStyle(el); return [parseFloat(css.borderLeftWidth) || 0, parseFloat(css.borderTopWidth) || 0]; }, null, { timeout: 500 }).catch(() => [0, 0]) : [0, 0];
        const point = box && (options?.position ? [box.x + border[0] + options.position.x, box.y + border[1] + options.position.y] : [box.x + box.width / 2, box.y + box.height / 2]);
        if (point && view && point[0] >= 0 && point[0] <= view.width && point[1] >= 0 && point[1] <= view.height) at = point;
      }
      if (at) {
        shown.cursor = at;
        await page.evaluate(([x, y]) => window.__walkthroughMove?.(x, y), at).catch(() => {});
        await page.waitForTimeout(demo ? 600 : 400);
        if (["click", "dblclick", "check", "uncheck", "tap"].includes(name)) await page.evaluate(([x, y]) => window.__walkthroughRipple?.(x, y), at).catch(() => {});
      }
      return original.apply(this, callArgs);
    };
  });
  const settle = (ms) => page.waitForTimeout(ms);
  // What the overlay shows is kept here too and drawn again on every page
  // that loads, since session storage is the origin's own: a link to another
  // origin would lose the caption, and coming back would show an old one.
  const shown = { caption: "", cursor: null };
  page.on("domcontentloaded", () => page.evaluate((state) => window.__walkthroughSet?.(state), shown).catch(() => {}));

  const step = async (text) => {
    log.push(`step: ${text}`);
    shown.caption = text;
    timeline.captions.push({ at: now(), text });
    await page.evaluate((caption) => {
      window.__walkthroughHighlight?.(null);
      window.__walkthroughSet?.({ caption });
    }, text).catch(() => {});
    await settle(demo ? 2200 : 1200);
  };
  const highlight = async (locator) => {
    const handle = await locator.elementHandle();
    if ((await handle.ownerFrame()) === page.mainFrame()) await page.evaluate((el) => window.__walkthroughHighlight?.(el), handle);
    else await page.evaluate((rect) => window.__walkthroughHighlight?.(null, rect), await locator.boundingBox());
    await settle(demo ? 1500 : 600);
  };
  // In demo pace, an expect that fails is noted and the walk goes on, so the
  // video still shows the rest; in test pace it stops the run, as before.
  const failures = [];
  // Every matcher, the ones under .not, .resolves and .rejects too.
  const lenient = (target) =>
    new Proxy(target, {
      get(on, name) {
        const value = on[name];
        if (value && typeof value === "object") return lenient(value);
        if (typeof value !== "function") return value;
        return async (...callArgs) => {
          try {
            return await value.apply(on, callArgs);
          } catch (error) {
            const message = error.message.split("\n")[0];
            failures.push(message);
            log.push(`expect failed: ${message}`);
          }
        };
      },
    });
  // expect itself keeps its own helpers (expect.any and the rest); the
  // expects it makes (configure, extend, soft) are lenient as it is, and so
  // are expect.poll's matchers.
  const leniently = (made) =>
    new Proxy(made, {
      apply: (target, self, expectArgs) => lenient(target.apply(self, expectArgs)),
      get: (target, name) => {
        const value = target[name];
        if (name === "poll") return (...pollArgs) => lenient(value.apply(target, pollArgs));
        if (["configure", "extend"].includes(name)) return (...madeArgs) => leniently(value.apply(target, madeArgs));
        if (name === "soft") return leniently(value);
        return typeof value === "function" ? value.bind(target) : value;
      },
    });
  const expect = demo
    ? leniently(playwrightExpect)
    : playwrightExpect;

  const result = { passed: true, error: null };
  try {
    await page.goto(url);
    await require(path.resolve(stepsFile))({ page, step, highlight, expect });
    await settle(1500);
  } catch (error) {
    result.passed = false;
    result.error = error.message.split("\n")[0];
    log.push(`failed: ${error.message}`);
  }
  if (failures.length) {
    result.passed = false;
    result.error ??= failures[0];
  }
  const screenshot = path.join(out, "end.png");
  await page.screenshot({ path: screenshot }).catch(() => {});
  const recording = page.video();
  await context.close();

  const name = videoName();
  const webm = path.join(out, `${name}.webm`);
  fs.renameSync(await recording.path(), webm);
  fs.rmSync(raw, { recursive: true, force: true });
  // QuickTime and Slack play mp4, not webm. The address bar and captions are
  // laid round it (frame); when that can't be done, the plain recording, said
  // so in the log.
  let video = webm;
  const mp4 = path.join(out, `${name}.mp4`);
  if (await frame(timeline, webm, mp4, log)) {
    fs.rmSync(webm);
    video = mp4;
  } else {
    log.push("no address bar or captions: they couldn't be laid round the video");
    try {
      execFileSync("ffmpeg", ["-y", "-loglevel", "error", "-i", webm, "-c:v", "libx264", "-pix_fmt", "yuv420p", "-movflags", "+faststart", mp4]);
      fs.rmSync(webm);
      video = mp4;
    } catch {}
  }
  // A frame every two seconds, five across, for the first minute: the video
  // looked over at once.
  let contact = null;
  try {
    const sheet = path.join(out, "contact.png");
    execFileSync("ffmpeg", ["-y", "-loglevel", "error", "-i", video, "-vf", "fps=1/2,scale=320:-1,tile=5x6:padding=6:color=white", "-frames:v", "1", sheet]);
    contact = sheet;
  } catch {}
  const logFile = path.join(out, "log.txt");
  fs.writeFileSync(logFile, `${log.join("\n")}\n`);
  console.log(JSON.stringify({ ...result, video, contact, screenshot, log: logFile, timeline }, null, 2));
};

(login ? signIn() : record()).catch((error) => {
  console.error(error.message);
  process.exit(1);
});
