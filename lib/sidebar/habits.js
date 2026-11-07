// Part of bin/tmux-sidebar; see the comment at the top of that file.
//
// How you use the workspace, counted on this Mac only and never sent
// anywhere, for suggestions in the sidebar's notifications (the habits
// setting): a setting to turn off that you never use, a key for a command you
// keep picking from M-p, and a feature that is off which what you do suggests.
//
// The counts are kept a file a day, habits/<YYYY-MM-DD>.tsv in the state
// directory, each line "<kind>\t<name>\t<count>", summed as read: `key` a key
// of the root table pressed (tmux.conf's `used`, taken from @used_keys once a
// minute), `palette` a command picked in M-p (tmux/palette.sh, its key as
// palette.sh presses it, so a root key picked there counts as a key press too
// and is taken off it) and `view` a mission control view brought to the front.
// Files over KEEP_DAYS old are removed. What a suggestion's menu put off or
// away is kept in suggestions.json beside them: an id and until when, 0 for
// good.

const fs = require("node:fs");
const path = require("node:path");
const { STATE_DIR } = require("./config");

const DIR = () => path.join(STATE_DIR, "habits");
const DISMISSED = () => path.join(STATE_DIR, "suggestions.json");
const KEEP_DAYS = 60;
const DAY_MS = 24 * 60 * 60_000;
const NOT_NOW_MS = 14 * DAY_MS;

const dayOf = (t) => {
  const d = new Date(t);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
};

// Counts to add to today's file: [{ kind, name, n }].
const record = (counts, now = Date.now()) => {
  const lines = counts.filter((c) => c.n > 0 && !/[\t\n]/.test(c.name)).map((c) => `${c.kind}\t${c.name}\t${c.n}\n`).join("");
  if (!lines) return;
  fs.mkdirSync(DIR(), { recursive: true, mode: 0o700 });
  fs.appendFileSync(path.join(DIR(), `${dayOf(now)}.tsv`), lines);
};

// The days kept, oldest first, with old ones removed.
const days = (now = Date.now()) => {
  let files;
  try {
    files = fs.readdirSync(DIR()).filter((f) => /^\d{4}-\d{2}-\d{2}\.tsv$/.test(f)).sort();
  } catch {
    return [];
  }
  const oldest = dayOf(now - KEEP_DAYS * DAY_MS);
  return files.filter((f) => {
    if (f.slice(0, 10) >= oldest) return true;
    fs.rmSync(path.join(DIR(), f), { force: true });
    return false;
  });
};

// The counts of the last `span` days, today included, as "<kind>\t<name>" →
// count, and how many days back the counting started.
const tally = (span, now = Date.now()) => {
  const files = days(now);
  const from = dayOf(now - (span - 1) * DAY_MS);
  const counts = new Map();
  files.filter((f) => f.slice(0, 10) >= from).forEach((f) => {
    let text = "";
    try {
      text = fs.readFileSync(path.join(DIR(), f), "utf8");
    } catch {}
    text.split("\n").forEach((line) => {
      const [kind, name, n] = line.split("\t");
      if (!name || !Number(n)) return;
      counts.set(`${kind}\t${name}`, (counts.get(`${kind}\t${name}`) ?? 0) + Number(n));
    });
  });
  const tracked = files.length ? Math.round((Date.parse(`${dayOf(now)}T00:00`) - Date.parse(`${files[0].slice(0, 10)}T00:00`)) / DAY_MS) + 1 : 0;
  return { counts, tracked };
};

// @used_keys as tmux holds it ("M-d,M-a,M-d,") into one count per key.
const keyCounts = (used) => {
  const counts = new Map();
  String(used ?? "").split(",").filter(Boolean).forEach((key) => counts.set(key, (counts.get(key) ?? 0) + 1));
  return [...counts].map(([name, n]) => ({ kind: "key", name, n }));
};

const readDismissed = () => {
  try {
    const saved = JSON.parse(fs.readFileSync(DISMISSED(), "utf8"));
    return saved && typeof saved === "object" && !Array.isArray(saved) ? saved : {};
  } catch {
    return {};
  }
};
// `forGood` puts it away for good, else for NOT_NOW_MS.
const dismissSuggestion = (id, forGood, now = Date.now()) => {
  const saved = { ...readDismissed(), [id]: forGood ? 0 : now + NOT_NOW_MS };
  fs.mkdirSync(STATE_DIR, { recursive: true, mode: 0o700 });
  fs.writeFileSync(`${DISMISSED()}.tmp`, JSON.stringify(saved));
  fs.renameSync(`${DISMISSED()}.tmp`, DISMISSED());
};

// Mission control's views that have a setting of their own, by their title,
// and whether turning one off also stops what it looks up in the background
// (the Calendar's feed and Slack's look don't wait on their views).
const VIEWS = [
  { title: "Work map", key: "view_work_map" },
  { title: "Calendar", key: "view_calendar" },
  { title: "Slack", key: "view_slack" },
  { title: "Closed", key: "view_closed" },
  { title: "PRs", key: "pr_board", needs: "github", lookups: true },
  { title: "CI", key: "view_ci", needs: "github", lookups: true },
  { title: "Landing", key: "view_landing", lookups: true },
  { title: "Tickets", key: "ticket_board", needs: "linear", lookups: true },
  { title: "Inbox", key: "view_inbox", needs: "linear", lookups: true },
  { title: "Sessions", key: "view_sessions" },
  { title: "Resources", key: "view_resources" },
  { title: "Usage", key: "usage" },
  { title: "Keys", key: "view_keys" },
];
const UNUSED_DAYS = 30;
const PICKS = 3;
const PICK_DAYS = 14;

// The features that are off by default, each suggested once what you do
// within a week says it would help: `keys` pressed (or picked in M-p) at least
// `times` together.
const TRIES = [
  {
    key: "second_status",
    keys: ["M-v", "M-i", "M-r", "M-l", "@palette p", "@palette t"],
    times: 10,
    title: "Turn on the second status line?",
    body: (n) => `You looked up a tab's PR or ticket ${n} times this week. The second status line shows the tab's branch, PR and Claude session under the tabs all the time.`,
  },
  {
    key: "todo_note",
    keys: ["M-N", "M-e", "@palette N", "@palette z"],
    times: 10,
    title: "Turn on today's todo note?",
    body: (n) => `You went to Notes ${n} times this week. Each weekday morning a note Todos/Today's Todo <date> would gather your notes' unticked items, today's calendar, and your PRs, tickets and Slack. It sends your notes' unticked items to Claude (Sonnet) to write it.`,
  },
];

// The suggestions to make now, from the counts alone, so its test needs no
// tmux: `value(key)` a setting's value, `notes` the root keys' descriptions
// (key → what it does) and `dismissed` suggestions.json.
const suggestionsFrom = ({ month, fortnight, week, value, notes, dismissed, now = Date.now() }) => {
  const count = (t, kind, name) => t.counts.get(`${kind}\t${name}`) ?? 0;
  // A key picked in M-p is pressed by it too, so it counts once.
  const pressed = (t, key) => Math.max(count(t, "key", key), count(t, "palette", key));
  const unused = month.tracked >= UNUSED_DAYS
    ? VIEWS.filter((v) => value(v.key) !== false && (!v.needs || value(v.needs) !== false) && !count(month, "view", v.title)).map((v) => ({
        id: `off:${v.key}`,
        title: `Turn off the ${v.title} view?`,
        body: `You haven't opened mission control's ${v.title} view in ${UNUSED_DAYS} days.${v.lookups ? " Turning it off also stops what it looks up in the background." : ""} Settings turns it on again.`,
        setting: { key: v.key, value: false, label: "Turn it off" },
      }))
    : [];
  const keys = Object.entries(notes)
    .map(([key, note]) => ({ key, note, n: count(fortnight, "palette", key) }))
    .filter((k) => k.n >= PICKS)
    .sort((a, b) => b.n - a.n)
    .map((k) => ({
      id: `key:${k.key}`,
      title: `${k.key}: ${k.note.replace(/\s+#.*$/, "")}`,
      body: `You picked this from M-p ${k.n} times in ${PICK_DAYS} days. ${k.key} does it in one press, from any pane.`,
    }));
  const tries = TRIES.filter((t) => value(t.key) === false)
    .map((t) => ({ t, n: t.keys.reduce((sum, key) => sum + pressed(week, key), 0) }))
    .filter(({ t, n }) => n >= t.times)
    .map(({ t, n }) => ({ id: `on:${t.key}`, title: t.title, body: t.body(n), setting: { key: t.key, value: true, label: "Turn it on" } }));
  return [...keys, ...tries, ...unused].filter((s) => !(s.id in dismissed) || (dismissed[s.id] !== 0 && dismissed[s.id] <= now));
};

const suggestions = ({ value, notes, now = Date.now() }) =>
  suggestionsFrom({ month: tally(UNUSED_DAYS, now), fortnight: tally(PICK_DAYS, now), week: tally(7, now), value, notes, dismissed: readDismissed(), now });

// With the habits setting off, nothing counted is kept: the days and what
// the suggestions' menus put off or away, so turning it on starts afresh.
const forget = () => {
  fs.rmSync(DIR(), { recursive: true, force: true });
  fs.rmSync(DISMISSED(), { force: true });
};

module.exports = { record, tally, prune: days, forget, keyCounts, suggestions, suggestionsFrom, dismissSuggestion, VIEWS };
