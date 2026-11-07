// Tests for mission control's Keys view (lib/sidebar/keymap.js, M-/), and
// that lib/sidebar/app-keys.json lists every key M-d's diff and Notes map,
// and nothing they don't. Run with: node --test test/*.test.js
//
// The settings file is a temporary one, so your own settings aren't read.
// M-d's keys are read from its lazy.nvim spec and Notes' from a headless
// `nvim -u NONE` running the plugin on a temporary notes directory, as
// test/notes.test.js does.

const { test, after } = require("node:test");
const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "keymap-")));
after(() => fs.rmSync(tmp, { recursive: true, force: true }));
process.env.SWITCHBOARD_SETTINGS = path.join(tmp, "settings.json");
const { keysView, parseList, vimKey } = require("../lib/sidebar/keymap");
const APP_KEYS = require("../lib/sidebar/app-keys.json");

const list = parseList(
  [
    "M-H\tM-S-H\tSplit pane to the left #panes #split",
    "M-=\tM-=\tZoom the pane in or out #panes #layout",
    "M-p\tM-p\tShow this list #palette",
    "@palette D\t\tCopy the diff's story (M-d) as Markdown #git #diff #diffview",
    "@palette S\t\tSettings: turn the workspace's options on and off #settings #config",
    "C-b x\tC-b x\tKill the pane #panes",
  ].join("\n"),
);

test("tmux's keys are grouped by their first tag, the biggest group first, each sorted by what it does", () => {
  const items = keysView(list).items;
  const tmuxPart = items.slice(0, items.findIndex((i) => i.title === "Sidebar"));
  assert.deepEqual(
    tmuxPart.map((i) => (i.group ? `[${i.title} ${i.count}]` : `${i.keyText} ${i.title}`)),
    ["[panes 3]", "C-b x Kill the pane", "M-S-H Split pane to the left", "M-= Zoom the pane in or out", "[git 1]", "M-p Copy the diff's story (M-d) as Markdown", "[palette 1]", "M-p Show this list", "[settings 1]", "M-p Settings: turn the workspace's options on and off"],
    "a palette-only command is shown as reached through M-p",
  );
  const zoom = items.find((i) => i.title === "Zoom the pane in or out");
  assert.equal(zoom.detail, "#panes #layout", "its tags beside it");
  assert.match(zoom.search, /M-= #panes #layout/, "the / filter matches its key and every tag");
  assert.equal(zoom.press, "M-=");
  assert.equal(items.find((i) => i.title.startsWith("Settings:")).press, "@palette S", "a palette command pressed through its table");
  assert.equal(items.find((i) => i.title.startsWith("Copy the diff")).press, undefined, "M-d's commands only in the diff, so not pressed from here");
});

test("then the sidebar's, mission control's, M-d's and Notes' own keys, each a group", () => {
  const groups = keysView(list).items.filter((i) => i.group).map((i) => i.title);
  assert.deepEqual(groups.slice(-8), ["Sidebar", "Mission control", "M-d's diff", "M-d: the cover while the story is told", "M-d: asking a question (ga)", "M-d: an answer (ga)", "Notes: a note", "Notes: the list"]);
  const items = keysView([]).items;
  assert.ok(items.some((i) => i.keyText === "? " || i.keyText === "?"), "the sidebar's help rows");
  assert.ok(items.some((i) => i.keyText === "space b" && i.title.startsWith("bold")), "Notes' leader keys with the leader as space, a row each");
  assert.ok(items.some((i) => i.keyText === "tab ⇧tab"), "Shift-Tab as the sidebar's help writes it");
  assert.ok(items.some((i) => i.keyText === "ga"), "Ask, with diff_ask on by default");
  fs.writeFileSync(process.env.SWITCHBOARD_SETTINGS, JSON.stringify({ diff_ask: false }));
  require("../lib/settings").reloadSettings();
  assert.ok(!keysView([]).items.some((i) => i.keyText === "ga" || i.title === "M-d: an answer (ga)"), "and not with it off, nor the answer's keys");
  assert.ok(!items.some((i) => i.keyText === "⌘S"), "easy mode's Cmd keys only with easy_keys on");
  fs.rmSync(process.env.SWITCHBOARD_SETTINGS);
  require("../lib/settings").reloadSettings();
  assert.deepEqual(["<CR>", "<Esc>", "<C-f>", "<C-S-p>", "<Tab>", "]c"].map(vimKey), ["↵", "esc", "^f", "^⇧p", "tab", "]c"]);
});

const nvim = (code, opts = {}) => execFileSync("nvim", ["--headless", "-u", "NONE", "-i", "NONE", ...(opts.args ?? []), "-l", "/dev/stdin"], { input: code, encoding: "utf8", ...opts }).trim().split("\n").at(-1);
const listed = (where) => APP_KEYS.find((g) => g.in === where).keys.flatMap((k) => k.keys.map((key) => ({ key, mode: k.mode ?? "n" })));

test("app-keys.json lists every key M-d's keymaps add, and each one it lists is mapped there", () => {
  const nvimDir = path.join(__dirname, "../nvim");
  const out = nvim(
    `
      local spec = dofile("${nvimDir}/lua/plugins/diffview.lua")
      local seen = {}
      for _, maps in pairs(spec.opts.keymaps) do for _, m in ipairs(maps) do seen[m[1] .. " " .. m[2]] = true end end
      io.write(vim.json.encode(vim.tbl_keys(seen)))
    `,
    { args: ["--cmd", `set rtp^=${nvimDir}`] },
  );
  // Vim reads <Esc> and <esc> alike.
  const norm = (key) => key.replace(/<[^>]+>/g, (k) => k.toLowerCase());
  const mapped = new Set(JSON.parse(out).map(norm));
  // Ask is added to them only with diff_ask on, as the spec's config does.
  const source = fs.readFileSync(path.join(nvimDir, "lua/plugins/diffview.lua"), "utf8");
  listed("diffview").forEach(({ key, mode }) => {
    const ok = mapped.has(norm(`${mode} ${key}`)) || (key === "ga" && /local ask = \{ "n", "ga"/.test(source));
    assert.ok(ok, `${key} is mapped in M-d's diff`);
  });
  // And every key its keymaps add is listed, whatever the mode.
  const named = new Set(listed("diffview").map(({ key }) => norm(key)));
  [...mapped].map((m) => m.split(" ")[1]).forEach((key) => assert.ok(named.has(key), `${key}, in M-d's keymaps, is in app-keys.json`));
  // The answer window's (ga), each mapped by its own map("<key>", …) there,
  // the only calls written so.
  const answer = [...source.matchAll(/^  map\("([^"]+)"/gm)].map((m) => norm(m[1]));
  const answerListed = listed("diffask").map(({ key }) => norm(key));
  assert.deepEqual([...new Set(answerListed)].sort(), [...new Set(answer)].sort(), "the answer window's keys, all of them and no others");
  // The story's cover and the question box ga opens map theirs with
  // vim.keymap.set in a function of their own: a key named there, or in a
  // `for _, key in ipairs({ … })` around it.
  const region = (start) => source.slice(source.indexOf(start), source.indexOf("\nend\n", source.indexOf(start)));
  const keysIn = (code) => [...code.matchAll(/vim\.keymap\.set\([^,]+,\s*"([^"]+)"/g), ...[...code.matchAll(/for _, key in ipairs\(\{([^}]*)\}\)/g)].flatMap((m) => [...m[1].matchAll(/"([^"]+)"/g)])].map((m) => norm(m[1]));
  for (const [where, start] of [["diff-cover", "local function cover(view)"], ["diffask-box", "ask_box = function(view)"]]) {
    const found = [...new Set(keysIn(region(start)))].sort();
    assert.ok(found.length, `${start} found`);
    assert.deepEqual([...new Set(listed(where).map(({ key }) => norm(key)))].sort(), found, `${where}'s keys, all of them and no others`);
  }
});

test("app-keys.json lists every key Notes maps in a note and in its list, and each one it lists is mapped", () => {
  const dir = fs.mkdtempSync(path.join(tmp, "notes-"));
  const home = path.join(tmp, "home");
  fs.mkdirSync(home, { recursive: true });
  const env = { ...process.env, HOME: home, NOTES_DIR: dir, XDG_CONFIG_HOME: path.join(home, ".config"), XDG_DATA_HOME: path.join(home, ".local/share"), XDG_STATE_HOME: path.join(home, ".local/state") };
  delete env.TMUX;
  // Each listed key as Vim names its map (maparg's lhs), "" when unmapped;
  // and every map the buffer has in Normal and Insert mode.
  const check = (keys) => `{ ${keys.map(({ key, mode }) => `{ ${JSON.stringify(key)}, vim.fn.maparg(${JSON.stringify(key)}, ${JSON.stringify(mode)}, false, true).lhs or "" }`).join(", ")} }`;
  const maps = "function(buf) local out = {} for _, m in ipairs({ \"n\", \"i\" }) do for _, k in ipairs(vim.api.nvim_buf_get_keymap(buf, m)) do out[#out + 1] = k.lhs end end return out end";
  const out = nvim(
    `
      vim.g.mapleader = " "
      vim.cmd("Notes")
      -- Easy mode's Cmd keys too, which its setting adds.
      require("notes.easy").keys(vim.api.nvim_get_current_buf())
      local s = require("notes.sidebar")
      local maps = ${maps}
      local note = { listed = ${check(listed("note"))}, maps = maps(vim.api.nvim_get_current_buf()) }
      vim.api.nvim_set_current_win(vim.fn.bufwinid(s.buf))
      local list = { listed = ${check(listed("notes-list"))}, maps = maps(s.buf) }
      io.write(vim.json.encode({ note = note, list = list }))
    `,
    { env, args: ["--cmd", `set rtp^=${path.join(__dirname, "../notes")}`, "--cmd", "runtime plugin/notes.lua"] },
  );
  const result = JSON.parse(out);
  // A click, and i, a and I, which only start typing after a line's marker,
  // aren't keys to learn.
  const unlisted = new Set(["<LeftRelease>", "i", "a", "I"]);
  for (const [where, { listed: keys, maps: mapped }] of Object.entries(result)) {
    keys.forEach(([key, lhs]) => assert.ok(lhs, `${key} is mapped in ${where}`));
    const named = new Set(keys.map(([, lhs]) => lhs.replace(/<Space>/gi, " ")));
    mapped.filter((lhs) => !unlisted.has(lhs)).forEach((lhs) => assert.ok(named.has(lhs), `${JSON.stringify(lhs)}, mapped in ${where}, is in app-keys.json`));
  }
});
