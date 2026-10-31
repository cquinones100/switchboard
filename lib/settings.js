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

// The ticket tracker was a Linear on/off option, `linear`, before Jira was
// one too: Linear turned off then is the tracker turned off now.
const migrated = (data) => {
  if (data.tracker === undefined && data.linear === false) data.tracker = "off";
  delete data.linear;
  return data;
};

const saved = () => {
  try {
    const data = JSON.parse(fs.readFileSync(SETTINGS_FILE, "utf8"));
    return data && typeof data === "object" && !Array.isArray(data) ? migrated(data) : {};
  } catch {
    return {};
  }
};

const valid = (option, value) => (option.choices ? option.choices.some((c) => c.value === value) : typeof value === "boolean");

// Every option with its value now: [{ ...option, value }].
const readSettings = () => {
  const mine = saved();
  return schema().map((option) => ({ ...option, value: valid(option, mine[option.key]) ? mine[option.key] : option.default }));
};

// One option's value now.
const setting = (key) => readSettings().find((option) => option.key === key)?.value;

// The settings as last read, for code that asks many times a second (the
// sidebar daemon draws several times a second): read again only by
// reloadSettings, which the daemon calls when the file changes.
let cache;
const currentSettings = () => (cache ??= readSettings());
const reloadSettings = () => (cache = readSettings());
// Whether an option is on, from the cache: an on/off one that isn't off, or
// one with choices not set to "off".
const isOn = (key) => {
  const value = currentSettings().find((option) => option.key === key)?.value;
  return value !== false && value !== "off";
};

// The value after this one: on/off flips, a choice goes to the next.
const nextValue = (option) => {
  if (!option.choices) return !option.value;
  const at = option.choices.findIndex((c) => c.value === option.value);
  return option.choices[(at + 1) % option.choices.length].value;
};

const valueWords = (option) => (option.choices ? option.choices.find((c) => c.value === option.value)?.label ?? String(option.value) : option.value ? "on" : "off");

// Keeps the file to what differs from the defaults, written to a temporary
// file and renamed into place, so a reader never sees half of it.
const writeSetting = (key, value) => {
  const option = schema().find((o) => o.key === key);
  if (!option || !valid(option, value)) throw new Error(`not a value for ${key}: ${JSON.stringify(value)}`);
  const mine = saved();
  if (value === option.default) delete mine[key];
  else mine[key] = value;
  fs.mkdirSync(path.dirname(SETTINGS_FILE), { recursive: true });
  const tmp = `${SETTINGS_FILE}.${process.pid}.tmp`;
  fs.writeFileSync(tmp, `${JSON.stringify(mine, null, 2)}\n`);
  fs.renameSync(tmp, SETTINGS_FILE);
};

module.exports = { SCHEMA_FILE, SETTINGS_FILE, readSettings, setting, currentSettings, reloadSettings, isOn, nextValue, valueWords, writeSetting };
