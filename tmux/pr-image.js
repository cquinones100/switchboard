#!/usr/bin/env node

// The PR view as GitHub draws it, as images in the pane: a trial beside
// tmux/pr-view.sh (M-v), which draws it as text. Run in a pane by
// tmux/pane-in-place.sh in place of the pane it is asked from, from the M-p
// palette, with passthrough on for that pane alone.
//
//   pr-image.js [<dir>]          the PR of <dir>'s branch (default: the current directory)
//   pr-image.js --ticket <ID>    ticket <ID> (M-p, in place of the pane, as the PR)
//
// A ticket comes from Linear or Jira, as the tracker setting says, with the
// key the ticket scripts use (lib/tracker): its title and the facts from its
// side panel, its description, sub-issues and comments, the Markdown drawn in the
// page by marked and cleaned by DOMPurify, since unlike GitHub's HTML nothing
// has made it safe, then laid out and paged as a PR is.
//
// The description comes as GitHub renders it (the pull request's body_html,
// read with a GET, so links, mentions and checklists are GitHub's own), into a
// page with GitHub's dark stylesheet and Mermaid drawing each diagram,
// opened in a headless Chrome: mermaid-cli's puppeteer, found from its global
// install, since this repository installs no packages for it. The page is
// screenshotted a pane-sized page at a time, at its own size (from the cell
// size the terminal reports), breaking between blocks, a diagram taller than
// a page drawn smaller to fit one, and each page shown through the
// kitty graphics protocol, which Ghostty speaks, passed through tmux (the
// pane's allow-passthrough). j, k, the arrows, Space and b page; g and G go
// to the ends; q closes it.
//
// Images in tmux are its weak point: tmux doesn't know they are there, so a
// tab switch can leave one stale until the page is drawn again, which r does.
// A resize lays the pages out again for the new size and draws the page that
// holds what was at the top.

const { execFile, execFileSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const CSS = "https://cdn.jsdelivr.net/npm/github-markdown-css@5.8.1/github-markdown-dark.css";
const MERMAID = "https://cdn.jsdelivr.net/npm/mermaid@11.4.1/dist/mermaid.min.js";
const MARKED = "https://cdn.jsdelivr.net/npm/marked@12.0.2/marked.min.js";
const PURIFY = "https://cdn.jsdelivr.net/npm/dompurify@3.1.6/dist/purify.min.js";
// How many device pixels to a CSS pixel: two on a Retina screen.
const DPR = 2;

const fail = (message) => {
  process.stdout.write(`\n  \x1b[31m${message}\x1b[0m\n\n  \x1b[38;5;244mPress any key to close.\x1b[0m`);
  process.stdin.setRawMode?.(true);
  process.stdin.once("data", () => process.exit(0));
};

const puppeteer = () => {
  try {
    const root = execFileSync("pnpm", ["root", "-g"], { encoding: "utf8" }).trim();
    // Its real path: pnpm links it in from its store, and puppeteer sits beside
    // it there.
    return require(require.resolve("puppeteer", { paths: [fs.realpathSync(path.join(root, "@mermaid-js/mermaid-cli"))] }));
  } catch {
    return undefined;
  }
};

const escapeHtml = (text) => text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

// The page: a heading (title, then its number or ID dimmed), a line of facts,
// and the body: GitHub's HTML, Mermaid blocks turned back into their source
// for Mermaid to draw, or, for a ticket, Markdown sources, each drawn into the
// element whose data-md names it.
const pageFor = ({ title, number, meta, html = "", markdown = [] }, width) => {
  // GitHub wraps a diagram's source in a section its own viewer fills in.
  const diagrams = html.replace(/<section[^>]*data-type="mermaid"[^>]*>[\s\S]*?<pre lang="mermaid"[^>]*>([\s\S]*?)<\/pre>[\s\S]*?<\/section>/g, (_, inner) => `<pre class="mermaid">${inner}</pre>`);
  return `<!doctype html><html><head><meta charset="utf-8"><link rel="stylesheet" href="${CSS}">
<style>body { background: #0d1117; margin: 0; } .markdown-body { box-sizing: border-box; width: ${width}px; padding: 24px; font-size: 14px; } pre.mermaid { background: transparent; }</style>
</head><body><article class="markdown-body">
<h1>${escapeHtml(title)} <span style="color:#7d8590">${escapeHtml(number)}</span></h1>
<p><em>${meta}</em></p>
${diagrams}
</article>
<script type="application/json" id="markdown">${JSON.stringify(markdown).replace(/</g, "\\u003c")}</script>
<script src="${MARKED}"></script>
<script src="${PURIFY}"></script>
<script src="${MERMAID}"></script>
<script>
const sources = JSON.parse(document.getElementById("markdown").textContent);
// In the element's place, so its paragraphs are the page's own blocks, which pages break between.
document.querySelectorAll("[data-md]").forEach((el) => { const t = document.createElement("template"); t.innerHTML = DOMPurify.sanitize(marked.parse(sources[Number(el.dataset.md)] ?? "")); el.replaceWith(t.content); });
document.querySelectorAll("pre > code.language-mermaid").forEach((code) => { const pre = document.createElement("pre"); pre.className = "mermaid"; pre.textContent = code.textContent; code.parentElement.replaceWith(pre); });
mermaid.initialize({ startOnLoad: false, theme: "dark" }); mermaid.run().finally(() => { document.body.dataset.done = "1"; });
</script>
</body></html>`;
};

// The marks kitty's Unicode placeholders carry for a cell's row and column,
// from kitty's gen/rowcolumn-diacritics.txt: the nth mark means row or
// column n.
const MARKS = [
  0x0305, 0x030D, 0x030E, 0x0310, 0x0312, 0x033D, 0x033E, 0x033F, 0x0346, 0x034A, 0x034B, 0x034C, 0x0350, 0x0351, 0x0352, 0x0357,
  0x035B, 0x0363, 0x0364, 0x0365, 0x0366, 0x0367, 0x0368, 0x0369, 0x036A, 0x036B, 0x036C, 0x036D, 0x036E, 0x036F, 0x0483, 0x0484,
  0x0485, 0x0486, 0x0487, 0x0592, 0x0593, 0x0594, 0x0595, 0x0597, 0x0598, 0x0599, 0x059C, 0x059D, 0x059E, 0x059F, 0x05A0, 0x05A1,
  0x05A8, 0x05A9, 0x05AB, 0x05AC, 0x05AF, 0x05C4, 0x0610, 0x0611, 0x0612, 0x0613, 0x0614, 0x0615, 0x0616, 0x0617, 0x0657, 0x0658,
  0x0659, 0x065A, 0x065B, 0x065D, 0x065E, 0x06D6, 0x06D7, 0x06D8, 0x06D9, 0x06DA, 0x06DB, 0x06DC, 0x06DF, 0x06E0, 0x06E1, 0x06E2,
  0x06E4, 0x06E7, 0x06E8, 0x06EB, 0x06EC, 0x0730, 0x0732, 0x0733, 0x0735, 0x0736, 0x073A, 0x073D, 0x073F, 0x0740, 0x0741, 0x0743,
  0x0745, 0x0747, 0x0749, 0x074A, 0x07EB, 0x07EC, 0x07ED, 0x07EE, 0x07EF, 0x07F0, 0x07F1, 0x07F3, 0x0816, 0x0817, 0x0818, 0x0819,
  0x081B, 0x081C, 0x081D, 0x081E, 0x081F, 0x0820, 0x0821, 0x0822, 0x0823, 0x0825, 0x0826, 0x0827, 0x0829, 0x082A, 0x082B, 0x082C,
  0x082D, 0x0951, 0x0953, 0x0954, 0x0F82, 0x0F83, 0x0F86, 0x0F87, 0x135D, 0x135E, 0x135F, 0x17DD, 0x193A, 0x1A17, 0x1A75, 0x1A76,
  0x1A77, 0x1A78, 0x1A79, 0x1A7A, 0x1A7B, 0x1A7C, 0x1B6B, 0x1B6D, 0x1B6E, 0x1B6F, 0x1B70, 0x1B71, 0x1B72, 0x1B73, 0x1CD0, 0x1CD1,
  0x1CD2, 0x1CDA, 0x1CDB, 0x1CE0, 0x1DC0, 0x1DC1, 0x1DC3, 0x1DC4, 0x1DC5, 0x1DC6, 0x1DC7, 0x1DC8, 0x1DC9, 0x1DCB, 0x1DCC, 0x1DD1,
  0x1DD2, 0x1DD3, 0x1DD4, 0x1DD5, 0x1DD6, 0x1DD7, 0x1DD8, 0x1DD9, 0x1DDA, 0x1DDB, 0x1DDC, 0x1DDD, 0x1DDE, 0x1DDF, 0x1DE0, 0x1DE1,
  0x1DE2, 0x1DE3, 0x1DE4, 0x1DE5, 0x1DE6, 0x1DFE, 0x20D0, 0x20D1, 0x20D4, 0x20D5, 0x20D6, 0x20D7, 0x20DB, 0x20DC, 0x20E1, 0x20E7,
  0x20E9, 0x20F0, 0x2CEF, 0x2CF0, 0x2CF1, 0x2DE0, 0x2DE1, 0x2DE2, 0x2DE3, 0x2DE4, 0x2DE5, 0x2DE6, 0x2DE7, 0x2DE8, 0x2DE9, 0x2DEA,
  0x2DEB, 0x2DEC, 0x2DED, 0x2DEE, 0x2DEF, 0x2DF0, 0x2DF1, 0x2DF2, 0x2DF3, 0x2DF4, 0x2DF5, 0x2DF6, 0x2DF7, 0x2DF8, 0x2DF9, 0x2DFA,
  0x2DFB, 0x2DFC, 0x2DFD, 0x2DFE, 0x2DFF, 0xA66F, 0xA67C, 0xA67D, 0xA6F0, 0xA6F1, 0xA8E0, 0xA8E1, 0xA8E2, 0xA8E3, 0xA8E4, 0xA8E5,
  0xA8E6, 0xA8E7, 0xA8E8, 0xA8E9, 0xA8EA, 0xA8EB, 0xA8EC, 0xA8ED, 0xA8EE, 0xA8EF, 0xA8F0, 0xA8F1, 0xAAB0, 0xAAB2, 0xAAB3, 0xAAB7,
  0xAAB8, 0xAABE, 0xAABF, 0xAAC1, 0xFE20, 0xFE21, 0xFE22, 0xFE23, 0xFE24, 0xFE25, 0xFE26, 0x10A0F, 0x10A38, 0x1D185, 0x1D186, 0x1D187,
  0x1D188, 0x1D189, 0x1D1AA, 0x1D1AB, 0x1D1AC, 0x1D1AD, 0x1D242, 0x1D243, 0x1D244,
];
// Its ID, which the placeholders carry as their 256-colour foreground: above
// 15, since tmux rewrites colours 0 to 15 as the basic ones, which carry no ID.
// Two of them, a page taking the one the page before it didn't, so every
// placeholder cell changes colour from one page to the next and tmux rewrites
// them all, rather than leaving some showing the page before.
const IMAGES = [222, 223];

// A kitty graphics image, shown through Unicode placeholders, which is what
// works inside tmux: the image is sent once (passed through tmux to the
// terminal, wrapped as tmux wants), with a virtual placement the size of the
// pane, and then drawn by printing placeholder characters, U+10EEEE with a
// row mark and a column mark, coloured with the image's ID. To tmux those are
// text, so they sit in this pane and move and redraw with it.
// `file`, when given, is the page saved on disk: sent as its path (t=f) the
// terminal reads it itself, a few dozen bytes through tmux rather than the
// whole image, which tmux relays slowly. Without one, the image itself.
const show = (png, cols, rows, file, image) => {
  const data = file ? Buffer.from(file).toString("base64") : png.toString("base64");
  const chunks = data.match(/.{1,4096}/g) ?? [];
  const medium = file ? "t=f," : "";
  const tmux = (seq) => (process.env.TMUX ? `\x1bPtmux;${seq.replace(/\x1b/g, "\x1b\x1b")}\x1b\\` : seq);
  // The screen is cleared first: clearing it after would delete the image
  // just sent along with what was on screen.
  let out = "\x1b[H\x1b[2J" + IMAGES.map((id) => tmux(`\x1b_Ga=d,d=I,i=${id},q=2\x1b\\`)).join("");
  chunks.forEach((chunk, i) => {
    const more = i < chunks.length - 1 ? 1 : 0;
    // Not quiet (q=1: errors only), so the footer can say why a page failed.
    const head = i === 0 ? `a=T,U=1,f=100,${medium}i=${image},c=${cols},r=${rows},q=1,m=${more}` : `m=${more}`;
    out += tmux(`\x1b_G${head};${chunk}\x1b\\`);
  });
  const cell = String.fromCodePoint(0x10eeee);
  out += `\x1b[38;5;${image}m`;
  // Every cell carries its own row and column, none worked out from the cell
  // before it: tmux redraws a changed screen a cell at a time, jumping about,
  // and a cell worked out from its neighbour then takes the wrong one's place.
  const across = Math.min(cols, MARKS.length);
  for (let r = 0; r < Math.min(rows, MARKS.length); r++) {
    const row = String.fromCodePoint(MARKS[r]);
    out += `\x1b[${r + 1};1H`;
    for (let c = 0; c < across; c++) out += `${cell}${row}${String.fromCodePoint(MARKS[c])}`;
  }
  process.stdout.write(`${out}\x1b[39m`);
};

// The terminal's cell size in device pixels: from tmux inside it, which knows
// it from the terminal (#{client_cell_width}); outside, from the terminal
// itself (CSI 16 t, answered as CSI 6 ; h ; w t). Undefined when neither says
// within a moment.
const cellSize = () =>
  new Promise((resolve) => {
    if (process.env.TMUX) {
      try {
        const [w, h] = execFileSync("tmux", ["display", "-p", ...(process.env.TMUX_PANE ? ["-t", process.env.TMUX_PANE] : []), "#{client_cell_width} #{client_cell_height}"], { encoding: "utf8" }).trim().split(" ").map(Number);
        if (w > 0 && h > 0) return resolve({ w, h });
      } catch {}
    }
    if (!process.stdin.isTTY) return resolve(undefined);
    process.stdin.setRawMode(true);
    const done = (size) => {
      clearTimeout(timer);
      process.stdin.off("data", onData);
      process.stdin.setRawMode(false);
      process.stdin.pause();
      resolve(size);
    };
    const onData = (data) => {
      const m = /\x1b\[6;(\d+);(\d+)t/.exec(data.toString());
      if (!m) return;
      done({ w: Number(m[2]), h: Number(m[1]) });
    };
    const timer = setTimeout(() => done(undefined), 500);
    process.stdin.on("data", onData);
    process.stdin.resume();
    process.stdout.write("\x1b[16t");
  });

// A ticket as the page's parts, or a reason it can't be: from Linear or Jira,
// as the tracker setting says, through lib/tracker, found from this file's
// real place, since tmux runs it through its link in ~/.config/tmux.
const ticketPage = async (id) => {
  const { tracker } = require(path.join(path.dirname(fs.realpathSync(__filename)), "../lib/tracker"));
  const provider = tracker();
  if (!provider) return { error: "The ticket tracker is off in Settings." };
  let issue;
  try {
    issue = await provider.detail(id);
    if (!issue) return { error: `${provider.name} has no ticket ${id}.` };
  } catch (err) {
    return { error: String(err.message).split("\n")[0] };
  }
  const markdown = [issue.description || "_No description._"];
  const facts = [issue.state?.name, issue.assignee?.name ?? "Unassigned", issue.priorityLabel, issue.project?.name, ...issue.labels.nodes.map((l) => l.name)].filter(Boolean);
  const children = issue.children.nodes.length
    ? `<h2>Sub-issues</h2><ul>${issue.children.nodes.map((c) => `<li><code>${escapeHtml(c.identifier)}</code> ${escapeHtml(c.title)} <span style="color:#7d8590">${escapeHtml(c.state?.name ?? "")}</span></li>`).join("")}</ul>`
    : "";
  const comments = [...issue.comments.nodes].sort((a, b) => Date.parse(a.createdAt) - Date.parse(b.createdAt));
  const thread = comments.length
    ? `<h2>Comments</h2>${comments.map((c) => {
        markdown.push(c.body ?? "");
        const who = c.user?.name ?? c.botActor?.name ?? "Someone";
        return `<h4>${escapeHtml(who)} <span style="color:#7d8590">${new Date(c.createdAt).toLocaleString("en-US", { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" })}</span></h4><div data-md="${markdown.length - 1}"></div>`;
      }).join("")}`
    : "";
  return { title: issue.title, number: issue.identifier, meta: facts.map(escapeHtml).join(" · "), html: `<div data-md="0"></div>${children}${thread}`, markdown };
};

// A PR as the page's parts, or a reason it can't be.
const gh = (args, options = {}) =>
  new Promise((resolve, reject) =>
    execFile("gh", args, { encoding: "utf8", maxBuffer: 16 * 1024 * 1024, ...options }, (err, stdout) => (err ? reject(err) : resolve(stdout))),
  );
const prPage = async (dir) => {
  let pr;
  try {
    pr = JSON.parse(await gh(["pr", "view", "--json", "number,title,url,state,isDraft,author,baseRefName,headRefName,body"], { cwd: dir }));
  } catch {
    return { error: "No PR for this branch." };
  }
  const repo = /github\.com\/([^/]+\/[^/]+)\/pull/.exec(pr.url)?.[1];
  let html;
  try {
    html = await gh(["api", "-H", "Accept: application/vnd.github.full+json", `repos/${repo}/pulls/${pr.number}`, "--jq", ".body_html // \"\""]);
  } catch {
    return { error: "Couldn't read the description as GitHub renders it." };
  }
  const meta = `${pr.isDraft ? "Draft" : pr.state} · ${escapeHtml(pr.author?.login ?? "")} wants to merge <code>${escapeHtml(pr.headRefName)}</code> into <code>${escapeHtml(pr.baseRefName)}</code>`;
  return { title: pr.title, number: `#${pr.number}`, meta, html };
};

const main = async () => {
  const ticket = process.argv[2] === "--ticket" ? process.argv[3] : undefined;
  const dir = path.resolve(ticket ? process.cwd() : process.argv[2] ?? process.cwd());
  // Taken first, before anything else reads the keyboard. A guess at a
  // 13-point font when the terminal doesn't say.
  let cell = (await cellSize()) ?? { w: 16, h: 34 };
  if (ticket && !/^[A-Za-z][A-Za-z0-9]*-\d+$/.test(ticket)) return fail("That isn't a ticket ID.");
  // The PR or ticket as last read is drawn at once, and read again while it
  // shows: when that has changed, the page is drawn again where it was.
  const gitDir = () => {
    try {
      return execFileSync("git", ["rev-parse", "--absolute-git-dir"], { cwd: dir, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
    } catch {
      return dir;
    }
  };
  const branch = () => {
    try {
      return execFileSync("git", ["branch", "--show-current"], { cwd: dir, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
    } catch {
      return "";
    }
  };
  const cache = path.join(process.env.XDG_CACHE_HOME || path.join(os.homedir(), ".cache"), "switchboard", "pr-image", `${(ticket ? ticket.toUpperCase() : `${gitDir()}:${branch()}`).replace(/\//g, "%")}.json`);
  const fresh = ticket ? ticketPage(ticket.toUpperCase()) : prPage(dir);
  const remember = (parts) => {
    fs.mkdirSync(path.dirname(cache), { recursive: true });
    fs.writeFileSync(cache, JSON.stringify(parts));
  };
  let parts;
  try {
    parts = JSON.parse(fs.readFileSync(cache, "utf8"));
  } catch {
    process.stdout.write(`\n  \x1b[38;5;244mReading the ${ticket ? "ticket" : "PR"}…\x1b[0m`);
    parts = await fresh;
    if (parts.error) return fail(parts.error);
    remember(parts);
  }
  const lib = puppeteer();
  if (!lib) return fail("This needs mermaid-cli's puppeteer: pnpm add -g @mermaid-js/mermaid-cli");

  process.stdout.write("\r  \x1b[38;5;244mDrawing it…\x1b[0m   ");
  const work = fs.mkdtempSync(path.join(os.tmpdir(), "pr-image-"));
  process.on("exit", () => fs.rmSync(work, { recursive: true, force: true }));
  const file = path.join(work, "pr.html");
  // The page is laid out as wide as the pane, in CSS pixels, and drawn at the
  // screen's own pixels, so it fills the pane at its own size, sharp. The
  // browser stays open, so a resize lays the same page out again rather than
  // reading and drawing it all over.
  // No wider or taller than the marks go, since a cell past the last has no
  // mark to say where it is, and that part of the image would never show.
  const size = () => {
    const cols = Math.min(process.stdout.columns || 80, MARKS.length);
    const rows = Math.min((process.stdout.rows || 24) - 1, MARKS.length);
    return { cols, rows, width: Math.max(480, Math.floor((cols * cell.w) / DPR)), pageHeight: Math.max(100, Math.floor((rows * cell.h) / DPR)) };
  };
  let view = size();
  const browser = await lib.launch({ headless: true });
  const page = await browser.newPage();
  const load = async () => {
    fs.writeFileSync(file, pageFor(parts, view.width));
    await page.setViewport({ width: view.width, height: 800, deviceScaleFactor: DPR });
    await page.goto(`file://${file}`, { waitUntil: "networkidle0" });
    await page.waitForSelector("body[data-done]", { timeout: 15_000 }).catch(() => {});
    // Filled in by layOut for each size; puppeteer refuses an empty one.
    const fitStyle = await page.addStyleTag({ content: "/* fit */" });
    await page.evaluate((el) => { el.dataset.fit = "1"; }, fitStyle);
  };
  await load();

  // The pages for the pane as it is now: each ends where a block does, and a
  // diagram taller than a page is drawn smaller, to fit one, so it is never
  // cut in two. Each layout's files get names of their own, so the terminal
  // never reads one the next layout is writing.
  let layouts = 0;
  const layOut = async () => {
    const { width, pageHeight, rows } = view;
    const n = ++layouts;
    await page.setViewport({ width, height: 800, deviceScaleFactor: DPR });
    await page.evaluate((w, h) => {
      document.querySelector(".markdown-body").style.width = `${w}px`;
      document.querySelectorAll("style").forEach((el) => {
        if (el.dataset.fit) el.textContent = `pre.mermaid svg { max-height: ${h - 40}px; max-width: 100%; height: auto; }`;
      });
    }, width, pageHeight);
    // Where each block of the description starts and ends, to break pages
    // between them.
    const { height, blocks } = await page.evaluate(() => {
      const body = document.querySelector(".markdown-body");
      const top = body.getBoundingClientRect().top;
      return {
        height: body.getBoundingClientRect().height,
        blocks: [...body.children].map((el) => ({ top: el.getBoundingClientRect().top - top, bottom: el.getBoundingClientRect().bottom - top })),
      };
    });
    const cuts = [0];
    while (cuts.at(-1) + pageHeight < height) {
      const start = cuts.at(-1);
      const fits = blocks.filter((b) => b.top > start && b.top <= start + pageHeight).map((b) => b.top);
      cuts.push(fits.length ? Math.max(...fits) : start + pageHeight);
    }
    cuts.push(height);
    const shots = [];
    for (let i = 0; i < cuts.length - 1; i++) {
      const h = cuts[i + 1] - cuts[i];
      // A Buffer: puppeteer returns a plain Uint8Array, whose toString ignores
      // "base64" and lists its bytes instead.
      const png = Buffer.from(await page.screenshot({ clip: { x: 0, y: cuts[i], width, height: h }, type: "png" }));
      const file = path.join(work, `page-${n}-${i + 1}.png`);
      fs.writeFileSync(file, png);
      // Where the page starts, as a share of the whole, to find it again in
      // the next layout.
      shots.push({ png, file, rows: Math.max(1, Math.round((h / pageHeight) * rows)), from: cuts[i] / height });
    }
    fs.readdirSync(work).filter((f) => f.startsWith("page-") && !f.startsWith(`page-${n}-`)).forEach((f) => fs.rmSync(path.join(work, f), { force: true }));
    return shots;
  };
  let shots = await layOut();
  // For checking the drawing without a terminal: the pages as PNGs, and out.
  if (process.env.PR_IMAGE_DUMP) {
    shots.forEach((shot, i) => fs.writeFileSync(path.join(process.env.PR_IMAGE_DUMP, `page-${i + 1}.png`), shot.png));
    await browser.close();
    process.exit(0);
  }

  let at = 0;
  // The terminal's last word on a page: kitty graphics answer an error as
  // ESC _G i=<id>;<message> ESC \.
  let said = "";
  // Pages go by file until the terminal turns one down, then as data.
  let byFile = true;
  // Which of the two image IDs the next drawing takes: every drawing takes
  // the other one from the drawing before, a redraw or resize as much as a
  // new page, so tmux rewrites every cell of it.
  let drawings = 0;
  // Set while a resize is laid out, when keys wait.
  let busy = false;
  const footer = (text) => process.stdout.write(`\x1b[${view.rows + 1};1H\x1b[2K\x1b[38;5;244m ${text}\x1b[0m`);
  const draw = () => {
    show(shots[at].png, view.cols, shots[at].rows, byFile ? shots[at].file : undefined, IMAGES[drawings++ % 2]);
    // A full redraw, as moving out of the pane and back makes, so nothing of
    // the page before is left on screen.
    if (process.env.TMUX) execFile("tmux", ["refresh-client"], () => {});
    footer(`page ${at + 1}/${shots.length}   j k space b: page   r: redraw   q: close${said ? `   \x1b[31m${said}` : ""}`);
  };
  // A resize lays the pages out again for the pane's new size, then shows
  // the page that holds where the one on screen started. A drag sends a run
  // of resizes, so it waits for them to stop, and one that comes while
  // laying out is laid out again after.
  let resizeTimer;
  let again = false;
  let reload = false;
  const relayout = async () => {
    if (busy) {
      again = true;
      return;
    }
    busy = true;
    if (reload) {
      reload = false;
      await load().catch(() => {});
    }
    const from = shots[at].from;
    // A new font size changes the cell; tmux knows it. Not cellSize(), whose
    // fallback asks the terminal on the keyboard's input and pauses it.
    if (process.env.TMUX) {
      try {
        const [w, h] = execFileSync("tmux", ["display", "-p", ...(process.env.TMUX_PANE ? ["-t", process.env.TMUX_PANE] : []), "#{client_cell_width} #{client_cell_height}"], { encoding: "utf8" }).trim().split(" ").map(Number);
        if (w > 0 && h > 0) cell = { w, h };
      } catch {}
    }
    view = size();
    process.stdout.write("\x1b[H\x1b[2J");
    footer("Laying it out for the new size…");
    try {
      shots = await layOut();
    } catch (err) {
      said = err.message.split("\n")[0];
    }
    at = Math.max(0, shots.findLastIndex((shot) => shot.from <= from + 1e-6));
    busy = false;
    if (again) {
      again = false;
      return relayout();
    }
    draw();
  };
  process.stdout.on("resize", () => {
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(relayout, 250);
  });
  process.stdin.setRawMode(true);
  process.stdin.on("data", (key) => {
    const k = key.toString();
    const reply = /\x1b_G([^\x1b]*)\x1b\\/.exec(k);
    if (reply) {
      if (byFile) {
        byFile = false;
        return draw();
      }
      said = reply[1];
      footer(`page ${at + 1}/${shots.length}   \x1b[31m${said}`);
      return;
    }
    if (k === "q" || k === "\x03" || k === "\x1b") {
      process.stdout.write("\x1b[2J");
      browser.close().finally(() => process.exit(0));
      return;
    }
    if (busy) return;
    if (["j", " ", "\x1b[B", "\x1b[6~"].includes(k)) at = Math.min(shots.length - 1, at + 1);
    else if (["k", "b", "\x1b[A", "\x1b[5~"].includes(k)) at = Math.max(0, at - 1);
    else if (k === "g") at = 0;
    else if (k === "G") at = shots.length - 1;
    draw();
  });
  // Read again: the cell size's question paused it, and a listener alone
  // doesn't undo a pause, so nothing would keep this running.
  process.stdin.resume();
  draw();
  const now = await fresh;
  if (now.error || JSON.stringify(now) === JSON.stringify(parts)) return;
  parts = now;
  remember(parts);
  reload = true;
  relayout();
};

main().catch((err) => fail(err.message.split("\n")[0]));
