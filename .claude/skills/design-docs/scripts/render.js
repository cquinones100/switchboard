#!/usr/bin/env node
// Draws the UX 2.0 canvas's boards (a claude.ai Design artifact's `project/<Name>.dc.html` files) as PNGs for docs/design,
// without the Design type's runtime (`support.js`, claude.ai's own, never copied here). A board is an `<x-dc>` template whose
// `<sc-for list="{{...}}" as="...">` loops and `{{holes}}` are filled from its script's `renderVals()`; this file evaluates that
// script in a `node:vm` context of its own (no require, process or file access, since the board is data read from the
// artifact), expands the template into plain HTML, and has the system's Chrome draw it headless at the board's size, under a
// Content-Security-Policy that runs no script and loads nothing but Google Fonts. It replaces the design tab's three
// converters (gridshot.js for the terminal-grid boards, slack/preview.js for the static Slack phone boards, indexmeasure's for
// the Index), since one expander covers the three shapes.
//
//   render.js <board.dc.html> <out.png> [width height]   draw one board (1280×800 unless given)
//   render.js sync <project dir> <docs/design dir>        check the boards' text, draw the boards that changed, write the README
//   render.js --self-test                                 draw the fixtures and check each PNG's size and that it isn't blank
'use strict';
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const vm = require('node:vm');
const crypto = require('node:crypto');
const zlib = require('node:zlib');
const { spawnSync } = require('node:child_process');

const CHROME = process.env.CHROME || ['/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', '/usr/bin/google-chrome', '/usr/bin/chromium'].find((p) => fs.existsSync(p));
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

// A board as a page Chrome can draw: its helmet's fonts and styles, and its template filled in, with no script.
function toHtml(source, values = renderValues(source)) {
  const dc = source.match(/<x-dc>([\s\S]*)<\/x-dc>/);
  if (!dc) throw new Error('no <x-dc> template');
  const helmet = withoutActive((dc[1].match(/<helmet>([\s\S]*?)<\/helmet>/) || ['', ''])[1]);
  const body = withoutActive(expand(dc[1].replace(/<helmet>[\s\S]*?<\/helmet>/, ''), values));
  return `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="${CSP}">${helmet}</head><body>${body}</body></html>`;
}

// The text a board's page shows, for the made-up-data check: the template's and every value filled into it.
function boardText(html) {
  // What a page shows besides its text nodes: the values of fields and the like, quoted or not, and the text CSS draws
  // (`content:`).
  const shown = [...html.matchAll(/\b(?:value|placeholder|title|alt|aria-label|label|data-[\w-]+)\s*=\s*("[^"]*"|'[^']*'|[^\s"'=<>`]+)/gi), ...html.matchAll(/\bcontent\s*:\s*("[^"]*"|'[^']*')/gi)].map((m) => m[1].replace(/^(["'])([\s\S]*)\1$/, '$2'));
  return (html + ' ' + shown.join(' ')).replace(/<style[\s\S]*?<\/style>/gi, ' ').replace(/<[^>]+>/g, ' ').replace(/&amp;/g, '&').replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&quot;/g, '"').replace(/\s+/g, ' ');
}

function draw(html, out, width, height) {
  if (!CHROME) throw new Error('no Chrome found: set CHROME to its path');
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'design-docs-'));
  try {
    const page = path.join(dir, 'board.html');
    fs.writeFileSync(page, html);
    // Chrome can exit nonzero as it tears down after writing the picture (its teardown watchdog), so the picture is what
    // says whether it drew.
    fs.rmSync(out, { force: true });
    const run = spawnSync(CHROME, ['--headless', '--disable-gpu', '--no-first-run', '--disable-background-networking', '--disable-component-update', '--hide-scrollbars', '--force-device-scale-factor=1', `--user-data-dir=${path.join(dir, 'profile')}`, '--virtual-time-budget=10000', `--window-size=${width},${height}`, `--screenshot=${path.resolve(out)}`, `file://${page}`], { encoding: 'utf8', timeout: 60000 });
    if (!fs.existsSync(out)) throw new Error(`Chrome drew nothing (exit ${run.status}): ${(run.error?.message || run.stderr || '').trim().split('\n').slice(-3).join(' ')}`);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

// A PNG's size, and whether its pixels are more than one color, read from its own chunks.
function pngFacts(file) {
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
  // A picture of one color, however its rows are filtered, holds only its pixel's few bytes, zeros and the filter types;
  // a drawn board holds most of the 256.
  const raw = zlib.inflateSync(Buffer.concat(idat));
  const distinct = new Set(raw).size;
  return { width, height, blank: distinct <= 12 };
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
  for (const [, who] of text.matchAll(/@([A-Za-z][\w.-]*)/g)) if (!MADE_UP.names.includes(who) && !MADE_UP.handles.includes(who)) found.add('@' + who);
  for (const [home] of text.matchAll(/\/(?:Users|home)\/[\w.-]+/g)) found.add(home);
  for (const [org] of text.matchAll(/github\.com\/([\w.-]+)/g)) if (!MADE_UP.organizations.includes(org.split('/')[1])) found.add(org);
  for (const [, host] of text.matchAll(/\b((?:[a-z0-9-]+\.)+(?:com|org|io|net|dev|app|co))\b/g)) if (!MADE_UP.hosts.includes(host)) found.add(host);
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

const RENDERER = fs.readFileSync(__filename);

function sync(projectDir, docsDir) {
  const canvas = JSON.parse(fs.readFileSync(path.join(projectDir, 'canvas.json'), 'utf8'));
  const version = process.env.CANVAS_VERSION || 'unknown';
  const boards = Object.entries(canvas.boards).filter(([, b]) => b.page === 'design').map(([file, b]) => {
    const number = (b.title.match(/^(\w+)\. /) || [])[1] || '';
    return { file, number, title: b.title.replace(/^\w+\. /, ''), width: b.w, height: b.h, png: `${number ? number + '-' : ''}${slugOf(file)}.png` };
  });
  const sources = Object.fromEntries(boards.map((b) => [b.file, fs.readFileSync(path.join(projectDir, b.file), 'utf8')]));

  const words = ownWords();
  // Each board is expanded once, and the page that passed the check is the one drawn, since its script could answer
  // differently a second time.
  const values = Object.fromEntries(boards.map((b) => [b.file, renderValues(sources[b.file])]));
  const pages = Object.fromEntries(boards.map((b) => [b.file, toHtml(sources[b.file], values[b.file])]));
  // The README is public too, made of the canvas's title, the boards' titles and the Index's groups, so it is checked with
  // the pages, before anything is written.
  const next = readme(canvas, boards, values, version);
  const found = [
    ...boards.flatMap((b) => leaks(boardText(pages[b.file]), words).map((text) => `${b.file}: ${text}`)),
    ...leaks(next.replace(/\(([\w.-]+\.png)\)/g, ''), words).map((text) => `README.md: ${text}`),
  ];
  if (found.length) {
    console.error('Not made-up data, to change on the canvas (or add to made-up.json when it is made up):\n' + found.join('\n'));
    process.exit(2);
  }

  fs.mkdirSync(docsDir, { recursive: true });
  const manifestPath = path.join(docsDir, 'boards.json');
  const before = fs.existsSync(manifestPath) ? JSON.parse(fs.readFileSync(manifestPath, 'utf8')) : {};
  const after = {};
  let drew = 0;
  for (const b of boards) {
    // What a picture is drawn from: the board, its size, and the renderer itself, so a fix here redraws every board.
    const hash = crypto.createHash('sha256').update(sources[b.file]).update(`\n${b.width}x${b.height}\n`).update(RENDERER).digest('hex');
    after[b.file] = { png: b.png, sha256: hash };
    if (before[b.file]?.sha256 === hash && before[b.file].png === b.png && fs.existsSync(path.join(docsDir, b.png))) continue;
    draw(pages[b.file], path.join(docsDir, b.png), b.width, b.height);
    console.log('drew ' + b.png);
    drew++;
  }
  // A picture goes only when no board now draws to its name: a board renamed with the same number keeps it.
  const kept = new Set(Object.values(after).map((entry) => entry.png));
  for (const old of Object.values(before)) {
    if (kept.has(old.png)) continue;
    fs.rmSync(path.join(docsDir, old.png), { force: true });
    console.log('removed ' + old.png);
    drew++;
  }
  fs.writeFileSync(manifestPath, JSON.stringify(after, null, 2) + '\n');
  // The version and date move only when a board or the README's text changed, so a sync with nothing new changes nothing.
  const readmePath = path.join(docsDir, 'README.md');
  const withoutTaken = (text) => text.replace(/Taken from the canvas's version .*? on \d{4}-\d{2}-\d{2}/, '');
  const unchanged = !drew && fs.existsSync(readmePath) && withoutTaken(fs.readFileSync(readmePath, 'utf8')) === withoutTaken(next);
  if (!unchanged) fs.writeFileSync(readmePath, next);
}

// The README: the boards grouped as the canvas's Index groups them, any board the Index leaves out under "Other boards".
function readme(canvas, boards, values, version) {
  const index = boards.find((b) => b.file === 'Index.dc.html');
  const groups = index ? values[index.file].groups || [] : [];
  const byFile = Object.fromEntries(boards.map((b) => [b.file, b]));
  const listed = new Set(index ? [index.file] : []);
  const entry = (b) => `### ${b.number ? b.number + '. ' : ''}${b.title}\n\n![${b.title}](${b.png})\n`;
  const sections = groups.map((group) => {
    const members = group.boards.map((gb) => byFile[gb.file]).filter(Boolean);
    members.forEach((b) => listed.add(b.file));
    const heading = group.label.charAt(0) + group.label.slice(1).toLowerCase();
    return `## ${heading}\n\n${group.about ? group.about.charAt(0).toUpperCase() + group.about.slice(1) + '.\n\n' : ''}${members.map(entry).join('\n')}`;
  });
  const others = boards.filter((b) => !listed.has(b.file));
  if (others.length) sections.push(`## Other boards\n\n${others.map(entry).join('\n')}`);
  const today = new Date().toISOString().slice(0, 10);
  return `# The design of UX 2.0\n\nThe boards of the UX 2.0 design canvas, "${canvas.title}", drawn as pictures: each one a screen of the workspace (160 × 45 cells) or of Slack on a phone, with made-up data. Taken from the canvas's version ${version} on ${today} by the \`design-docs\` project skill (\`.claude/skills/design-docs/\`), which redraws the boards that changed; don't edit these files by hand.\n\n${index ? `![The index of every board](${index.png})\n\n` : ''}${sections.join('\n')}`;
}

function selfTest() {
  if (!CHROME) {
    console.log('skipped: no Chrome found');
    process.exit(77);
  }
  const fixtures = path.join(__dirname, 'fixtures');
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'design-docs-test-'));
  let failed = false;
  try {
    for (const [file, width, height] of [['grid.dc.html', 1280, 800], ['slack.dc.html', 420, 880]]) {
      const out = path.join(dir, file + '.png');
      draw(toHtml(fs.readFileSync(path.join(fixtures, file), 'utf8')), out, width, height);
      const facts = pngFacts(out);
      const ok = facts.width === width && facts.height === height && !facts.blank;
      if (!ok) failed = true;
      console.log(`${ok ? 'ok' : 'FAILED'} ${file}: ${facts.width}×${facts.height}${facts.blank ? ', blank' : ''}`);
    }
    const leaked = leaks('Ana mentioned you · WEB-12 · #web-app · Zed mentioned you · ACME-4 · /Users/someone', []);
    const expected = ['Zed', 'ACME-4', '/Users/someone'];
    const leakOk = expected.every((t) => leaked.includes(t)) && leaked.length === expected.length;
    if (!leakOk) failed = true;
    console.log(`${leakOk ? 'ok' : 'FAILED'} made-up check: ${JSON.stringify(leaked)}`);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
  process.exit(failed ? 1 : 0);
}

module.exports = { toHtml, boardText, leaks, pngFacts };

const [first, second, third, fourth] = process.argv.slice(2);
if (require.main !== module) {
  // Required by another script: nothing to run.
} else if (first === '--self-test') selfTest();
else if (first === 'sync' && second && third) sync(second, third);
else if (first && second) draw(toHtml(fs.readFileSync(first, 'utf8')), second, Number(third) || 1280, Number(fourth) || 800);
else {
  console.error('usage: render.js <board.dc.html> <out.png> [width height] | render.js sync <project dir> <docs dir> | render.js --self-test');
  process.exit(64);
}
