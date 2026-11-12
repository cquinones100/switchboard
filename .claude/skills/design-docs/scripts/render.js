#!/usr/bin/env node
// Writes the UX 2.0 canvas's boards (a claude.ai Design artifact's `project/<Name>.dc.html` files) as static web pages for
// docs/design, which GitHub Pages serves, without the Design type's runtime (`support.js`, claude.ai's own, never copied here). A board is an `<x-dc>` template whose
// `<sc-for list="{{...}}" as="...">` loops and `{{holes}}` are filled from its script's `renderVals()`; this file evaluates that
// script in a `node:vm` context of its own (no require, process or file access, since the board is data read from the
// artifact) and expands the template into plain HTML under a Content-Security-Policy that runs no script and loads nothing but
// Google Fonts; the system's Chrome draws one headless at the board's size, for a look before landing and for the self-test. It replaces the design tab's three
// converters (gridshot.js for the terminal-grid boards, slack/preview.js for the static Slack phone boards, indexmeasure's for
// the Index), since one expander covers the three shapes.
//
//   render.js <board.dc.html> <out.png> [width height]   draw one board (1280×800 unless given)
//   render.js sync <project dir> <docs/design dir>        check the boards' text, write every board's page and the README
//   render.js made-up <range>                             run `switchboard leak-check <range>` and print what made-up.json
//                                                         doesn't explain in docs/design (exit 2 when it couldn't look)
//   render.js --self-test                                 draw the fixtures and check their size, that they aren't blank, and
//                                                         that a grid board's meter sits on its text's row
//   render.js --print-chrome-args                         print the switches Chrome draws every board with, a line each
'use strict';
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const vm = require('node:vm');
const zlib = require('node:zlib');
const { spawnSync } = require('node:child_process');

const CHROME = process.env.CHROME || ['/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', '/usr/bin/google-chrome', '/usr/bin/chromium'].find((p) => fs.existsSync(p));
const SITE = 'https://cquinones100.github.io/switchboard/design/';
const CSP = "default-src 'none'; script-src 'none'; style-src 'unsafe-inline' https://fonts.googleapis.com; font-src https://fonts.gstatic.com; img-src data:";

const escapeHtml = (text) => String(text).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');

// The values a board's script gives its template: what `new Component().renderVals()` returns, or nothing for a static board.
function renderValues(source) {
  const script = source.match(/<script type="text\/x-dc"[^>]*>([\s\S]*?)<\/script>/);
  if (!script || !/renderVals\s*\(/.test(script[1])) return {};
  const context = vm.createContext(Object.create(null));
  vm.runInContext('class DCLogic { constructor() { this.props = {}; this.state = {}; } }', context);
  // What the board throws is its own object, which printing it could run, so only a message string leaves the context.
  let values;
  try {
    values = vm.runInContext(script[1] + '\n;JSON.stringify(new Component().renderVals());', context, { timeout: 5000 });
  } catch (error) {
    let message = 'its script failed';
    try { message = String(vm.runInContext('(e) => String(e && e.message)', context)(error)).slice(0, 300); } catch {}
    throw new Error(`renderVals(): ${message}`);
  }
  if (typeof values !== 'string') throw new Error('renderVals() gave nothing');
  return JSON.parse(values);
}

function lookup(scope, name) {
  return name.split('.').reduce((value, key) => (value == null ? undefined : value[key]), scope);
}

// Expands `<sc-for>` (nested ones included) and `{{holes}}`, escaping every value filled in.
function expand(template, scope) {
  let out = '';
  let rest = template;
  for (;;) {
    const open = rest.match(/<sc-for\s+list="\{\{([\w.]+)\}\}"\s+as="(\w+)"[^>]*>/);
    if (!open) break;
    out += fillHoles(rest.slice(0, open.index), scope);
    const bodyStart = open.index + open[0].length;
    let depth = 1;
    let at = bodyStart;
    while (depth > 0) {
      const next = rest.slice(at).search(/<sc-for[\s>]|<\/sc-for>/);
      if (next < 0) throw new Error('an <sc-for> is never closed');
      at += next;
      if (rest.startsWith('</sc-for>', at)) { depth--; if (depth === 0) break; at += 9; } else { depth++; at += 7; }
    }
    const body = rest.slice(bodyStart, at);
    for (const item of lookup(scope, open[1]) || []) out += expand(body, { ...scope, [open[2]]: item });
    rest = rest.slice(at + 9);
  }
  return out + fillHoles(rest, scope);
}

const fillHoles = (text, scope) => text.replace(/\{\{([\w.]+)\}\}/g, (_, name) => escapeHtml(lookup(scope, name) ?? ''));

// What could run or take Chrome elsewhere, which the CSP alone doesn't stop: scripts, a <meta> refresh (a navigation, say to
// a file of yours drawn into a public PNG), <base>, frames and embeds.
// Removed until nothing changes, since taking one tag out can join the text around it into another.
const withoutActive = (html) => {
  for (;;) {
    const next = stripActive(html);
    if (next === html) return html;
    html = next;
  }
};
const stripActive = (html) => html.replace(/<script[\s\S]*?<\/script>/gi, '').replace(/<(?:iframe|object|embed|frameset)[\s\S]*?<\/(?:iframe|object|embed|frameset)>/gi, '').replace(/<(?:meta|base|iframe|frame|object|embed)\b[^>]*>/gi, '');

// A grid board draws its rows in a `white-space: pre` container, where the newlines between the template's tags are text: a
// blank line above the rows that would push them a row below the absolutely placed meters and pictures. The Slack boards and
// the Index are ordinary HTML, where a space between two tags can be a word's, so they keep theirs.
const isGrid = (source) => /white-space\s*:\s*pre\s*[;}"']/.test(source);

// A board as a page: its helmet's fonts and styles, and its template filled in, with no script. For the site, `page` gives
// its title and where each board's link (`<Name>.dc.html` on the canvas) goes.
function toHtml(source, values = renderValues(source), page = { title: '', links: null }) {
  const dc = source.match(/<x-dc>([\s\S]*)<\/x-dc>/);
  if (!dc) throw new Error('no <x-dc> template');
  const helmet = withoutActive((dc[1].match(/<helmet>([\s\S]*?)<\/helmet>/) || ['', ''])[1]);
  const template = dc[1].replace(/<helmet>[\s\S]*?<\/helmet>/, '');
  let body = withoutActive(expand(isGrid(source) ? template.trim().replace(/>\s+</g, '><') : template, values));
  if (page.links) body = body.replace(/\bhref="([\w.-]+\.dc\.html)"/g, (_, file) => `href="${page.links[file] || 'index.html'}"`);
  const head = page.title ? `<title>${escapeHtml(page.title)}</title><style>html{background:#0b0c12}</style>` : '';
  return `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="${CSP}">${head}${helmet}</head><body>${body}</body></html>`;
}

// The text a board's page holds, for the made-up-data check: what it shows (the template's text and every value filled into
// it, entities decoded) and, since the page is published whole, its source too, its styles and every attribute, with only its
// colors (`#c9cdd9`, which would read as channels) taken out.
function boardText(html) {
  // Besides its text nodes: the values of fields and the like, quoted or not, where its links and resources point, and the
  // text and addresses its CSS holds (`content:`, `url()`).
  const shown = [...html.matchAll(/\b(?:value|placeholder|title|alt|aria-label|label|href|src|srcset|action|cite|data-[\w-]+)\s*=\s*("[^"]*"|'[^']*'|[^\s"'=<>`]+)/gi), ...html.matchAll(/\bcontent\s*:\s*("[^"]*"|'[^']*')/gi), ...html.matchAll(/\burl\(\s*("[^"]*"|'[^']*'|[^)]*)\)/gi)].map((m) => m[1].replace(/^(["'])([\s\S]*)\1$/, '$2'));
  return (html.replace(/<style[\s\S]*?<\/style>/gi, ' ') + ' ' + shown.join(' ')).replace(/<[^>]+>/g, ' ').replace(/&amp;/g, '&').replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&quot;/g, '"').replace(/\s+/g, ' ') + ' ' + html.replace(/#[0-9a-fA-F]{3,8}(?![\w-])/g, ' ');
}

// The switches every board is drawn with. A new profile without --use-mock-keychain and --password-store=basic asks macOS
// for a keychain, which shows its Keychain Not Found box over whatever you are doing; Switchboard's own headless Chromes
// pass both too (src/views/image/render.rs's SWITCHES).
const SWITCHES = ['--headless', '--disable-gpu', '--no-first-run', '--use-mock-keychain', '--password-store=basic', '--disable-background-networking', '--disable-component-update', '--hide-scrollbars', '--force-device-scale-factor=1'];

function draw(html, out, width, height) {
  if (!CHROME) throw new Error('no Chrome found: set CHROME to its path');
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'design-docs-'));
  try {
    const page = path.join(dir, 'board.html');
    fs.writeFileSync(page, html);
    // Chrome can exit nonzero as it tears down after writing the picture (its teardown watchdog), so the picture is what
    // says whether it drew.
    fs.rmSync(out, { force: true });
    const run = spawnSync(CHROME, [...SWITCHES, `--user-data-dir=${path.join(dir, 'profile')}`, '--virtual-time-budget=10000', `--window-size=${width},${height}`, `--screenshot=${path.resolve(out)}`, `file://${page}`], { encoding: 'utf8', timeout: 60000 });
    if (!fs.existsSync(out)) throw new Error(`Chrome drew nothing (exit ${run.status}): ${(run.error?.message || run.stderr || '').trim().split('\n').slice(-3).join(' ')}`);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

// A PNG's size, and whether its pixels are more than one color, read from its own chunks.
function pngFacts(file) {
  const { width, height, raw } = pngChunks(file);
  // A picture of one color, however its rows are filtered, holds only its pixel's few bytes, zeros and the filter types;
  // a drawn board holds most of the 256.
  return { width, height, blank: new Set(raw).size <= 12 };
}

function pngChunks(file) {
  const png = fs.readFileSync(file);
  const width = png.readUInt32BE(16);
  const height = png.readUInt32BE(20);
  const idat = [];
  for (let at = 8; at < png.length;) {
    const length = png.readUInt32BE(at);
    const type = png.toString('latin1', at + 4, at + 8);
    if (type === 'IDAT') idat.push(png.subarray(at + 8, at + 8 + length));
    at += 12 + length;
  }
  return { width, height, depth: png[24], colorType: png[25], raw: zlib.inflateSync(Buffer.concat(idat)) };
}

// The rows of a PNG (8 bits a channel, RGB or RGBA, as Chrome writes) that hold a pixel of exactly `[r, g, b]`, with PNG's
// five row filters undone.
function rowsWithColor(file, [r, g, b]) {
  const { width, height, depth, colorType, raw } = pngChunks(file);
  const channels = { 2: 3, 6: 4 }[colorType];
  if (depth !== 8 || !channels) throw new Error(`a PNG of depth ${depth} and color type ${colorType}`);
  const stride = width * channels;
  let previous = Buffer.alloc(stride);
  const rows = [];
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    const line = Buffer.from(raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1)));
    for (let x = 0; x < stride; x++) {
      const left = x >= channels ? line[x - channels] : 0;
      const up = previous[x];
      const upLeft = x >= channels ? previous[x - channels] : 0;
      const p = left + up - upLeft;
      const paeth = Math.abs(p - left) <= Math.abs(p - up) && Math.abs(p - left) <= Math.abs(p - upLeft) ? left : Math.abs(p - up) <= Math.abs(p - upLeft) ? up : upLeft;
      line[x] = (line[x] + [0, left, up, (left + up) >> 1, paeth][filter]) & 0xff;
    }
    for (let x = 0; x < stride; x += channels) if (line[x] === r && line[x + 1] === g && line[x + 2] === b) { rows.push(y); break; }
    previous = line;
  }
  return rows;
}

// ─── The made-up-data check ────────────────────────────────────────────────────
// `leak-check` reads text, never PNGs, so before a picture is taken each board's text is held to the canvas's own made-up
// names; anything else stops the sync, naming the board and the text, to be changed on the canvas.
const MADE_UP = JSON.parse(fs.readFileSync(path.join(__dirname, 'made-up.json'), 'utf8'));

const DAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun', 'Today', 'Yesterday'];

function leaks(text, ownWords) {
  const found = new Set();
  for (const [, prefix, id] of text.matchAll(/\b([A-Z][A-Z0-9]{1,9})-(\d+)\b/g)) if (!MADE_UP.ticketPrefixes.includes(prefix)) found.add(`${prefix}-${id}`);
  for (const [channel] of text.matchAll(/(?<![\w&])#[a-z][a-z0-9_-]+/g)) if (!MADE_UP.channels.some((c) => c.startsWith(channel))) found.add(channel);
  // A CSS at-rule (`@media`) is the page's source, not a handle.
  const AT_RULES = ['charset', 'container', 'font-face', 'import', 'keyframes', 'layer', 'media', 'page', 'supports'];
  for (const [, who] of text.matchAll(/@([A-Za-z][\w.-]*)/g)) if (!AT_RULES.includes(who) && !MADE_UP.names.includes(who) && !MADE_UP.handles.includes(who)) found.add('@' + who);
  for (const [home] of text.matchAll(/\/(?:Users|home)\/[\w.-]+/g)) found.add(home);
  for (const [where, org] of text.matchAll(/(?:raw\.githubusercontent|github)\.com\/([\w.-]+)/g)) if (!MADE_UP.organizations.includes(org)) found.add(where);
  for (const [, host] of text.matchAll(/(?<![\w.-])((?:[a-z0-9-]+\.)+(?:com|org|io|net|dev|app|co))\b/g)) if (!MADE_UP.hosts.includes(host)) found.add(host);
  // Any address's host, whatever its suffix, protocol-relative ones (`//host/path`) included: what a link or a resource names is published with the page.
  for (const [, host] of text.matchAll(/(?:\b[a-z][a-z0-9+.-]*:)?\/\/([a-z0-9.-]+[a-z0-9])/gi)) if (!MADE_UP.hosts.includes(host.toLowerCase())) found.add(host);
  // A person's name on the canvas: before what they did ("Ana mentioned you"), after who did it ("approved by Ben"), or
  // as a Slack message's author before its time ("Alex 2:05 PM"). A truncated channel ("#onca") counts as the one it begins.
  const people = [/\b([A-Z][a-z]+)(?= (?:mentioned|replied|wrote|reacted|commented|assigned|reviewed|approved)\b)/g, /\b(?:approved|asked|assigned|reviewed|opened|mentioned|written|requested) by ([A-Z][a-z]+)/g, /\b([A-Z][a-z]+) \d{1,2}:\d{2} [AP]M\b/g];
  for (const pattern of people) for (const [, who] of text.matchAll(pattern)) if (!MADE_UP.names.includes(who) && !DAYS.includes(who)) found.add(who);
  const lower = text.toLowerCase();
  for (const word of ownWords) if (lower.includes(word.toLowerCase())) found.add(word);
  return [...found];
}

// The words of ~/.config/switchboard/leak-check.txt, which `switchboard leak-check` reads too: your own names and employer's.
function ownWords() {
  try {
    return fs.readFileSync(path.join(os.homedir(), '.config/switchboard/leak-check.txt'), 'utf8').split('\n').map((w) => w.trim()).filter((w) => w && !w.startsWith('#'));
  } catch {
    return [];
  }
}

// ─── The sync ──────────────────────────────────────────────────────────────────
const slugOf = (file) => file.replace(/\.dc\.html$/, '').replace(/([a-z0-9])([A-Z])/g, '$1-$2').toLowerCase();

function sync(projectDir, docsDir) {
  const canvas = JSON.parse(fs.readFileSync(path.join(projectDir, 'canvas.json'), 'utf8'));
  const version = process.env.CANVAS_VERSION || 'unknown';
  const boards = Object.entries(canvas.boards).filter(([, b]) => b.page === 'design').map(([file, b]) => {
    const number = (b.title.match(/^(\w+)\. /) || [])[1] || '';
    return { file, number, title: b.title.replace(/^\w+\. /, ''), page: `${number ? number + '-' : ''}${slugOf(file)}.html` };
  });
  const sources = Object.fromEntries(boards.map((b) => [b.file, fs.readFileSync(path.join(projectDir, b.file), 'utf8')]));
  const links = Object.fromEntries(boards.map((b) => [b.file, b.page]));

  const words = ownWords();
  // Each board is expanded once, and the page that passed the check is the one written, since its script could answer
  // differently a second time.
  const values = Object.fromEntries(boards.map((b) => [b.file, renderValues(sources[b.file])]));
  const pages = Object.fromEntries(boards.map((b) => [b.file, toHtml(sources[b.file], values[b.file], { title: `${b.number ? b.number + '. ' : ''}${b.title}`, links })]));
  // The README is public too, made of the canvas's title, the boards' titles and the Index's groups, so it is checked with
  // the pages, before anything is written.
  const next = readme(canvas, boards, values, version);
  const found = [
    ...boards.flatMap((b) => leaks(boardText(pages[b.file]), words).map((text) => `${b.file}: ${text}`)),
    ...leaks(next.split(SITE).join(''), words).map((text) => `README.md: ${text}`),
  ];
  if (found.length) {
    console.error('Not made-up data, to change on the canvas (or add to made-up.json when it is made up):\n' + found.join('\n'));
    process.exit(2);
  }

  fs.mkdirSync(docsDir, { recursive: true });
  let changed = 0;
  for (const b of boards) {
    const file = path.join(docsDir, b.page);
    if (fs.existsSync(file) && fs.readFileSync(file, 'utf8') === pages[b.file]) continue;
    fs.writeFileSync(file, pages[b.file]);
    console.log('wrote ' + b.page);
    changed++;
  }
  // What the folder holds is this script's output alone, so a page no board writes now goes, with the pictures and their
  // manifest from when the boards were PNGs.
  const kept = new Set(boards.map((b) => b.page));
  for (const old of fs.readdirSync(docsDir)) {
    if (kept.has(old) || !(/\.(?:html|png)$/.test(old) || old === 'boards.json')) continue;
    fs.rmSync(path.join(docsDir, old), { force: true });
    console.log('removed ' + old);
    changed++;
  }
  // The version and date move only when a board or the README's text changed, so a sync with nothing new changes nothing.
  const readmePath = path.join(docsDir, 'README.md');
  const withoutTaken = (text) => text.replace(/Taken from the canvas's version .*? on \d{4}-\d{2}-\d{2}/, '');
  const unchanged = !changed && fs.existsSync(readmePath) && withoutTaken(fs.readFileSync(readmePath, 'utf8')) === withoutTaken(next);
  if (!unchanged) fs.writeFileSync(readmePath, next);
}

// The README: the site's address, then the boards grouped as the canvas's Index groups them, each its number and title linking
// to its page there, any board the Index leaves out under "Other boards".
function readme(canvas, boards, values, version) {
  const index = boards.find((b) => b.file === 'Index.dc.html');
  const groups = index ? values[index.file].groups || [] : [];
  const byFile = Object.fromEntries(boards.map((b) => [b.file, b]));
  const listed = new Set(index ? [index.file] : []);
  const entry = (b) => `- [${b.number ? b.number + '. ' : ''}${b.title.replace(/[\\[\]]/g, '\\$&')}](${SITE}${b.page})\n`;
  const sections = groups.map((group) => {
    const members = group.boards.map((gb) => byFile[gb.file]).filter(Boolean);
    members.forEach((b) => listed.add(b.file));
    const heading = group.label.charAt(0) + group.label.slice(1).toLowerCase();
    return `## ${heading}\n\n${group.about ? group.about.charAt(0).toUpperCase() + group.about.slice(1) + '.\n\n' : ''}${members.map(entry).join('')}`;
  });
  const others = boards.filter((b) => !listed.has(b.file));
  if (others.length) sections.push(`## Other boards\n\n${others.map(entry).join('')}`);
  const today = new Date().toISOString().slice(0, 10);
  return `# The design of UX 2.0\n\nThe boards of the UX 2.0 design canvas, "${canvas.title}", as web pages: each one a screen of the workspace (160 × 45 cells) or of Slack on a phone, with made-up data. GitHub Pages serves them at ${SITE}${index ? `, starting from [the index](${SITE}${index.page}), where every board links back to it` : ''}. Taken from the canvas's version ${version} on ${today} by the \`design-docs\` project skill (\`.claude/skills/design-docs/\`), which writes every page again; don't edit these files by hand.\n\n${sections.join('\n')}`;
}

// ─── leak-check's findings in the pages ────────────────────────────────────────
// `switchboard leak-check` reads the pages, where the canvas's made-up ticket IDs and PR numbers look like real ones and a
// six-digit color like a PR number. What made-up.json lists in docs/design (its ticket prefixes, its PR numbers, and the
// colors leak-check reads as PR numbers) is dropped, judged from the finding alone, since leak-check reads every commit of
// the range and a page as it is now may no longer hold what an earlier commit added. Every other finding is
// printed for the person to answer, and the exit is 1 when there is one. It runs leak-check itself, so a leak-check that
// couldn't look (its exit 2, or no exit at all) is passed on as it is, never read as a range with nothing in it.
function madeUp(range) {
  const run = spawnSync('switchboard', ['leak-check', range], { encoding: 'utf8' });
  if (run.status !== 0 && run.status !== 1) {
    process.stderr.write(run.stderr || `leak-check couldn't run: ${run.error?.message || 'it exited ' + run.status}\n`);
    process.exit(2);
  }
  const kept = run.stdout.split('\n').filter((line) => {
    if (!line.trim()) return false;
    const hit = line.match(/^docs\/design\/[^:]+:\d+: (ticket ID|PR number): (.*)$/);
    if (!hit) return true;
    const [, kind, text] = hit;
    if (kind === 'ticket ID') return !MADE_UP.ticketPrefixes.includes(text.split('-')[0]);
    return !MADE_UP.prNumbers.includes(Number(text.slice(1))) && !MADE_UP.colors.includes(text.slice(1));
  });
  if (kept.length) console.log(kept.join('\n'));
  process.exit(kept.length ? 1 : 0);
}

function selfTest() {
  let failed = false;
  const check = (ok, what) => {
    if (!ok) failed = true;
    console.log(`${ok ? 'ok' : 'FAILED'} ${what}`);
  };
  const fixtures = path.join(__dirname, 'fixtures');
  const grid = toHtml(fs.readFileSync(path.join(fixtures, 'grid.dc.html'), 'utf8'));
  const slack = toHtml(fs.readFileSync(path.join(fixtures, 'slack.dc.html'), 'utf8'));
  check(!/>\s*\n\s*</.test(grid.slice(grid.indexOf('<body>'))), 'grid page: no line breaks between its tags');
  check(/<\/b> 9:40 AM/.test(slack), 'slack page: its spaces between tags kept');
  const leaked = leaks('Ana mentioned you · WEB-12 · #web-app · Zed mentioned you · ACME-4 · /Users/someone', []);
  const expected = ['Zed', 'ACME-4', '/Users/someone'];
  check(expected.every((t) => leaked.includes(t)) && leaked.length === expected.length, `made-up check: ${JSON.stringify(leaked)}`);
  if (!CHROME) {
    console.log('skipped drawing: no Chrome found');
    process.exit(failed ? 1 : 77);
  }
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'design-docs-test-'));
  try {
    for (const [file, html, width, height] of [['grid.dc.html', grid, 1280, 800], ['slack.dc.html', slack, 420, 880]]) {
      const out = path.join(dir, file + '.png');
      draw(html, out, width, height);
      const facts = pngFacts(out);
      check(facts.width === width && facts.height === height && !facts.blank, `${file}: ${facts.width}×${facts.height}${facts.blank ? ', blank' : ''}`);
      if (file !== 'grid.dc.html') continue;
      // The fixture's fourth row is "meter:" on #ff00ff with its meter, #00ff00, placed on that row: a row's shift moves the
      // text off the meter.
      const text = rowsWithColor(out, [0xff, 0x00, 0xff]);
      const meter = rowsWithColor(out, [0x00, 0xff, 0x00]);
      check(text.length > 0 && meter.length > 0 && text[0] === meter[0] && text.at(-1) === meter.at(-1), `grid meter on its text's row: text ${text[0]}–${text.at(-1)}, meter ${meter[0]}–${meter.at(-1)}`);
    }
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
  process.exit(failed ? 1 : 0);
}

module.exports = { toHtml, boardText, leaks, pngFacts, rowsWithColor };

const [first, second, third, fourth] = process.argv.slice(2);
if (require.main !== module) {
  // Required by another script: nothing to run.
} else if (first === '--self-test') selfTest();
else if (first === '--print-chrome-args') console.log(SWITCHES.join('\n'));
else if (first === 'sync' && second && third) sync(second, third);
else if (first === 'made-up' && second) madeUp(second);
else if (first && second) draw(toHtml(fs.readFileSync(first, 'utf8')), second, Number(third) || 1280, Number(fourth) || 800);
else {
  console.error('usage: render.js <board.dc.html> <out.png> [width height] | render.js sync <project dir> <docs dir> | render.js made-up <range> | render.js --self-test | render.js --print-chrome-args');
  process.exit(64);
}
