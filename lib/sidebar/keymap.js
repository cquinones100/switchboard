// Part of bin/tmux-sidebar; see the comment at the top of that file.
//
// Mission control's Keys view (M-/): every key Switchboard binds, grouped.
// First tmux's, as the M-p palette lists them (`palette.sh --list`: the root,
// prefix and palette tables' bindings tmux.conf describes), each under the
// first of its tags, the biggest group first; then the sidebar's and mission
// control's own (render.js's HELP, which their ? shows), and M-d's diff's and
// Notes' (app-keys.json, which test/keymap.test.js checks against their maps).
// Its / filter matches a key, its words and every tag. Enter on a tmux key
// presses it in the pane mission control was opened over, as M-p does.
//
// tmux's bindings are read in the background as the daemon starts and as
// the view is come to (refreshKeys), since they change only when tmux.conf
// is loaded again, and drawing reads only what was read last.

const os = require("node:os");
const path = require("node:path");
const { execFile } = require("node:child_process");
const { HELP } = require("./render");
const { isOn } = require("../settings");
const APP_KEYS = require("./app-keys.json");

const PALETTE = path.join(os.homedir(), ".config/tmux/palette.sh");

// [{ keys, shown, description }], as palette.sh --list last gave them.
let bindings = [];
const parseList = (stdout) =>
  stdout.split("\n").filter(Boolean).map((line) => {
    const [keys, shown, description] = line.split("\t");
    return { keys, shown, description: description ?? "" };
  });
const refreshKeys = () =>
  new Promise((resolve) =>
    execFile("bash", [PALETTE, "--list"], { encoding: "utf8", timeout: 10_000, env: { ...process.env, LC_ALL: "en_US.UTF-8" } }, (err, stdout) => {
      if (!err) bindings = parseList(stdout);
      resolve();
    }),
  );

// A Vim key as the sidebar's help writes one: ↵, esc, ^f, and the leader as
// the space Switchboard's Neovim config makes it.
const vimKey = (key) =>
  key
    .replace(/^<leader>/i, "space ")
    .replace(/^<CR>$/i, "↵")
    .replace(/^<Esc>$/i, "esc")
    .replace(/^<Tab>$/i, "tab")
    .replace(/^<S-Tab>$/i, "⇧tab")
    .replace(/^<S-CR>$/i, "⇧↵")
    .replace(/^<C-S-(.)>$/i, "^⇧$1")
    .replace(/^<C-(.)>$/i, "^$1");

const header = (title, count) => ({ title, group: "category", count, state: "idle", detail: "" });

// "Split the pane below #panes #split" -> its words and its tags.
const split = (description) => {
  const at = description.indexOf(" #");
  return at < 0 ? { words: description, tags: [] } : { words: description.slice(0, at), tags: description.slice(at + 1).split(/\s+/).filter((t) => t.startsWith("#")) };
};

const tmuxItems = (list) => {
  const groups = new Map();
  list.forEach((b) => {
    const { words, tags } = split(b.description);
    const group = tags[0]?.slice(1) ?? "other";
    // A palette-only command is reached through M-p.
    const key = b.keys.startsWith("@palette ") ? "M-p" : b.shown;
    if (!groups.has(group)) groups.set(group, []);
    groups.get(group).push({
      title: words,
      detail: tags.join(" "),
      keyText: key,
      // M-d's commands type into the diff, so only it should get them.
      press: tags.includes("#diffview") ? undefined : b.keys,
      state: "idle",
      search: `${key} ${tags.join(" ")}`,
    });
  });
  return [...groups.entries()]
    .sort(([a, x], [b, y]) => y.length - x.length || a.localeCompare(b))
    .flatMap(([group, items]) => [header(group, items.length), ...items.sort((a, b) => a.title.localeCompare(b.title))]);
};

const helpItems = (title, rows) => [header(title, rows.length), ...rows.map(([keys, does]) => ({ title: does, keyText: keys, detail: "", state: "idle", search: `${title} ${keys}` }))];

const appItems = () =>
  APP_KEYS.filter((g) => !g.setting || isOn(g.setting)).flatMap(({ group, keys }) => {
    // A key the terminal sends for Cmd (easy mode) is shown as the Cmd key.
    const rows = keys.filter((k) => !k.setting || isOn(k.setting)).map((k) => [k.shown ?? k.keys.map(vimKey).join(" "), k.does]);
    return helpItems(group, rows);
  });

const keysView = (list = bindings) => ({
  title: "Keys",
  items: [...tmuxItems(list), ...helpItems("Sidebar", HELP.sidebar), ...helpItems("Mission control", HELP.mission), ...appItems()],
  loaded: true,
  builtIn: true,
  grouped: true,
  keysView: true,
});

module.exports = { keysView, refreshKeys, parseList, vimKey };
