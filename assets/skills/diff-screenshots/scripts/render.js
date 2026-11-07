#!/usr/bin/env node
// Renders each file in a plan (from plan.js, with labels filled in) through
// delta, turns delta's ANSI colour into an HTML page, and screenshots it.
//
//   node render.js <plan.json> --out <dir> [--unified]
//
// Writes <dir>/html/<n>.html and <dir>/<n>.png, one image per file however
// tall, and prints the PNG paths as JSON.
//
// delta runs with the user's gitconfig, which sets side-by-side, line numbers
// and navigate. --width is at least 230, and wider for long lines (see
// widthFor), so side-by-side lines don't wrap (at 150, test files wrapped
// into an 11,700px image). --paging=never makes delta
// colour its output even into a pipe. delta has no way to turn side-by-side
// off but --no-gitconfig, which --unified passes.

const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");
const Convert = require("ansi-to-html");
const { PNG } = require("pngjs");

const VIEWPORT_WIDTH = 1900;
// One PNG per file, however long, so a file scrolls as one image. Chrome
// captures at most 16,384 device pixels in one go (taller, the end of the
// page goes missing), so a taller page is captured in slices and stitched.
// At 2x, that limit is about 465 lines of diff; a file longer than that is
// taken at 1x, which keeps the image light enough to scroll and under the
// relay's 20 MB for an image.
const SCALE = 2;
const TALL_SCALE = 1;
const MAX_CAPTURE = 16000; // device px, a little under Chrome's 16,384
// delta wraps any line longer than half its width in side-by-side. 230 is
// enough for most code; a code file with longer lines gets a width that fits
// its longest one, up to MAX_WIDTH, past which wrapping is the lesser evil.
// Docs stay at 230: a paragraph is one long line, and prose wraps fine.
const MIN_WIDTH = 230;
const MAX_WIDTH = 340;
const widthFor = (diff, sideBySide) => {
  const longest = diff
    .toString("utf8")
    .split("\n")
    .filter((line) => /^[ +-]/.test(line) && !/^(\+\+\+|---) /.test(line))
    .reduce((max, line) => Math.max(max, line.slice(1).replace(/\t/g, "    ").length), 0);
  // Each side also holds its line numbers and borders, about 12 columns.
  const wanted = sideBySide ? 2 * (longest + 12) : longest + 16;
  return String(Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, wanted)));
};

const args = process.argv.slice(2);
const planFile = args[0];
const out = args[args.indexOf("--out") + 1];
const unified = args.includes("--unified");
if (!planFile || !args.includes("--out") || !out) {
  console.error("usage: node render.js <plan.json> --out <dir> [--unified]");
  process.exit(2);
}
const plan = JSON.parse(fs.readFileSync(planFile, "utf8"));
fs.mkdirSync(path.join(out, "html"), { recursive: true });

const escape = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
// OSC 8 hyperlinks, which delta adds when the gitconfig turns them on, would
// show up as stray text in the page.
const stripOsc = (s) => s.replace(/\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)/g, "");
// delta packs background and foreground into one SGR (48;2;r;g;b;38;2;r;g;b),
// which ansi-to-html misreads as a bright green background; it needs one per code.
const splitSgr = (s) =>
  s.replace(/\x1b\[([\d;]*)m/g, (_, params) => {
    const codes = params.split(";");
    const groups = [];
    while (codes.length > 0) {
      const code = codes.shift();
      const extended = code === "38" || code === "48" ? codes.splice(0, codes[0] === "2" ? 4 : 2) : [];
      groups.push([code, ...extended].join(";"));
    }
    return groups.map((group) => `\x1b[${group}m`).join("");
  });

const pages = plan.files.map((file) => {
  const diff = execFileSync("git", ["-C", plan.repo, "diff", "-M", plan.range, "--", ...file.paths], { maxBuffer: 256 * 1024 * 1024 });
  const ansi = execFileSync("delta", [...(unified ? ["--no-gitconfig"] : []), "--paging=never", "--width", file.group === "docs" ? String(MIN_WIDTH) : widthFor(diff, !unified)], { input: diff, maxBuffer: 256 * 1024 * 1024 }).toString("utf8");
  const body = new Convert({ fg: "#d4d4d4", bg: "#1e1e1e", escapeXML: true }).toHtml(splitSgr(stripOsc(ansi)));
  const label = file.label || `${file.n}. ${file.paths.at(-1)}`;
  const html = `<!doctype html>
<html><head><meta charset="utf-8"><title>${escape(label)}</title>
<style>
  html, body { margin: 0; background: #1e1e1e; color: #d4d4d4; }
  body { padding: 16px 20px; width: max-content; }
  h2 { font: 600 18px/1.3 -apple-system, "Helvetica Neue", sans-serif; margin: 0 0 4px; }
  .path { font: 12px/1.3 Menlo, monospace; color: #8a8a8a; margin: 0 0 12px; }
  pre { font: 13px/1.35 Menlo, monospace; width: max-content; margin: 0; }
</style></head>
<body><h2>${escape(label)}</h2><p class="path">${escape(file.paths.join(" → "))}</p><pre>${body}</pre></body></html>
`;
  const htmlPath = path.join(out, "html", `${file.n}.html`);
  fs.writeFileSync(htmlPath, html);
  return { n: file.n, label, htmlPath };
});

// @playwright/test from the repo being diffed when it has one, else the
// playwright installed beside this script. The system Chrome, since
// Playwright's own browsers are usually not downloaded here.
const loadChromium = () => {
  try {
    return require(require.resolve("@playwright/test", { paths: [plan.repo, process.cwd()] })).chromium;
  } catch {}
  try {
    return require("playwright").chromium;
  } catch {
    console.error(`No Playwright found. Install it beside this script: npm i --save-exact playwright@1.63.0 --prefix ${__dirname}`);
    process.exit(3);
  }
};

(async () => {
  const chromium = loadChromium();
  const browser = await chromium.launch({ channel: "chrome" });
  // A page per scale, since a page's scale is fixed when it is opened.
  const tabs = {};
  const pageAt = async (scale) => (tabs[scale] ??= await browser.newPage({ viewport: { width: VIEWPORT_WIDTH, height: 900 }, deviceScaleFactor: scale }));
  const shots = [];
  for (const p of pages) {
    let page = await pageAt(SCALE);
    await page.goto(`file://${p.htmlPath}`);
    // The body, not the document: the document is never smaller than the
    // viewport, which would pad a short diff out to 900px and every one to
    // the viewport's width. A diff wider than the viewport still counts in full.
    const { width, height } = await page.evaluate(() => {
      const box = document.body.getBoundingClientRect();
      return { width: Math.ceil(Math.max(box.right, document.body.scrollWidth)), height: Math.ceil(box.bottom) };
    });
    const scale = height * SCALE <= MAX_CAPTURE ? SCALE : TALL_SCALE;
    if (scale !== SCALE) {
      page = await pageAt(scale);
      await page.goto(`file://${p.htmlPath}`);
    }
    const file = path.join(out, `${p.n}.png`);
    const slice = Math.floor(MAX_CAPTURE / scale);
    if (height <= slice) {
      await page.screenshot({ path: file, fullPage: true, clip: { x: 0, y: 0, width, height } });
    } else {
      // Slices of the same width, top to bottom: their pixel rows, one after
      // another, are the whole image's.
      const tops = Array.from({ length: Math.ceil(height / slice) }, (_, k) => k * slice);
      const rows = [];
      for (const y of tops) {
        const png = PNG.sync.read(await page.screenshot({ fullPage: true, clip: { x: 0, y, width, height: Math.min(slice, height - y) } }));
        rows.push({ png, data: png.data });
      }
      const whole = new PNG({ width: rows[0].png.width, height: rows.reduce((sum, r) => sum + r.png.height, 0) });
      whole.data = Buffer.concat(rows.map((r) => r.data));
      fs.writeFileSync(file, PNG.sync.write(whole));
    }
    shots.push({ n: p.n, label: p.label, png: file, width, height, scale });
  }
  await browser.close();
  process.stdout.write(JSON.stringify({ out: path.resolve(out), shots }, null, 2) + "\n");
})().catch((err) => {
  console.error(err.message);
  process.exit(1);
});
