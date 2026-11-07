// The tmux workspace's settings: every option is listed, with its category,
// description and default, in tmux/settings.json, and what you have changed
// is kept in ~/.config/switchboard/settings.json, only the keys you set,
// so an option you never touched follows its default. Mission control's
// Settings view changes them here, and tmux/settings.sh applies them to tmux,
// where the scripts read them as @setting_<key> options.
//
// An option is on/off (a boolean default) or one of its `choices`. A value in
// the file that is neither is ignored, and the default used.

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const SCHEMA_FILE = path.join(__dirname, "../tmux/settings.json");
const SETTINGS_FILE = process.env.SWITCHBOARD_SETTINGS || path.join(os.homedir(), ".config/switchboard/settings.json");

const schema = () => JSON.parse(fs.readFileSync(SCHEMA_FILE, "utf8"));

const saved = () => {
  try {
    const data = JSON.parse(fs.readFileSync(SETTINGS_FILE, "utf8"));
    return data && typeof data === "object" && !Array.isArray(data) ? data : {};
  } catch {
    return {};
  }
};

// Settings that were on/off and became choices, each old value read as the
// choice that means the same, until migrateSettings writes it over.
const REVALUED = { autopilot_close: { true: "seen", false: "off" } };
const revalued = (key, value) => (typeof value === "boolean" && REVALUED[key]?.[value]) || value;

const valid = (option, value) => (option.choices ? option.choices.some((c) => c.value === value) : typeof value === "boolean");

// Every option with its value now: [{ ...option, value }].
const readSettings = () => {
  const mine = saved();
  return schema().map((option) => {
    const value = revalued(option.key, mine[option.key]);
    return { ...option, value: valid(option, value) ? value : option.default };
  });
};

// One option's value now.
const setting = (key) => readSettings().find((option) => option.key === key)?.value;

// The settings as last read, for code that asks many times a second (the
// sidebar daemon draws several times a second): read again only by
// reloadSettings, which the daemon calls when the file changes.
let cache;
const currentSettings = () => (cache ??= readSettings());
const reloadSettings = () => (cache = readSettings());
// Whether an on/off option is on, from the cache.
const isOn = (key) => currentSettings().find((option) => option.key === key)?.value !== false;

// The value after this one: on/off flips, a choice goes to the next.
const nextValue = (option) => {
  if (!option.choices) return !option.value;
  const at = option.choices.findIndex((c) => c.value === option.value);
  return option.choices[(at + 1) % option.choices.length].value;
};

const valueWords = (option) => (option.choices ? option.choices.find((c) => c.value === option.value)?.label ?? String(option.value) : option.value ? "on" : "off");

// Keeps the file to what differs from the defaults, written to a temporary
// file and renamed into place, so a reader never sees half of it. A file
// that is there but can't be read as a JSON object is never written over,
// since starting from nothing would lose every other setting in it; the
// version before each write is kept beside it as settings.json.bak.
const writeSetting = (key, value) => {
  const option = schema().find((o) => o.key === key);
  if (!option || !valid(option, value)) throw new Error(`not a value for ${key}: ${JSON.stringify(value)}`);
  let mine = {};
  if (fs.existsSync(SETTINGS_FILE)) {
    try {
      mine = JSON.parse(fs.readFileSync(SETTINGS_FILE, "utf8"));
    } catch {
      mine = null;
    }
    if (!mine || typeof mine !== "object" || Array.isArray(mine)) {
      throw new Error(`${SETTINGS_FILE.replace(os.homedir(), "~")} can't be read as JSON; fix it by hand, then change the setting again`);
    }
    fs.copyFileSync(SETTINGS_FILE, `${SETTINGS_FILE}.bak`);
  }
  if (value === option.default) delete mine[key];
  else mine[key] = value;
  fs.mkdirSync(path.dirname(SETTINGS_FILE), { recursive: true });
  const tmp = `${SETTINGS_FILE}.${process.pid}.tmp`;
  fs.writeFileSync(tmp, `${JSON.stringify(mine, null, 2)}\n`);
  fs.renameSync(tmp, SETTINGS_FILE);
};

// Settings renamed since they were saved, each read once as its new key and
// then dropped from the file: the Merge queues view became the Landing view.
// An on/off value of a setting that became choices (REVALUED) is written as
// its choice, the default's dropped, so tmux/settings.sh reads it too.
const RENAMED = { view_merge_queues: "view_landing" };
const migrateSettings = () => {
  let mine;
  try {
    mine = JSON.parse(fs.readFileSync(SETTINGS_FILE, "utf8"));
  } catch {
    return;
  }
  if (!mine || typeof mine !== "object" || Array.isArray(mine)) return;
  const stale = Object.keys(REVALUED).filter((key) => typeof mine[key] === "boolean");
  if (!stale.length && !Object.keys(RENAMED).some((old) => old in mine)) return;
  Object.entries(RENAMED).forEach(([old, now]) => {
    if (!(old in mine)) return;
    if (!(now in mine) && mine[old] === false) mine[now] = false;
    delete mine[old];
  });
  const defaults = Object.fromEntries(schema().map((option) => [option.key, option.default]));
  stale.forEach((key) => {
    mine[key] = revalued(key, mine[key]);
    if (mine[key] === defaults[key]) delete mine[key];
  });
  fs.copyFileSync(SETTINGS_FILE, `${SETTINGS_FILE}.bak`);
  const tmp = `${SETTINGS_FILE}.${process.pid}.tmp`;
  fs.writeFileSync(tmp, `${JSON.stringify(mine, null, 2)}\n`);
  fs.renameSync(tmp, SETTINGS_FILE);
};

module.exports = { SCHEMA_FILE, migrateSettings, SETTINGS_FILE, readSettings, setting, currentSettings, reloadSettings, isOn, nextValue, valueWords, writeSetting };
