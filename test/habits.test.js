// Tests for lib/sidebar/habits.js, the counts behind the habits setting's
// suggestions. Run with: node --test test/*.test.js
//
// The state directory is a temporary one, so nothing reads or writes your
// own counts.

const { test, after } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "habits-"));
process.env.TMUX_SIDEBAR_STATE_DIR = tmp;
const habits = require("../lib/sidebar/habits");
after(() => fs.rmSync(tmp, { recursive: true, force: true }));

const DAY = 24 * 60 * 60_000;
const NOW = Date.parse("2026-11-05T12:00");
const on = () => true;
const notes = { "M-d": "Open the branch's diff #git #diff", "M-a": "Go to the next tab that needs you #tabs" };

test("counts from @used_keys and the day files are summed over the days asked for", () => {
  assert.deepEqual(habits.keyCounts("M-d,M-a,M-d,"), [{ kind: "key", name: "M-d", n: 2 }, { kind: "key", name: "M-a", n: 1 }]);
  assert.deepEqual(habits.keyCounts(undefined), []);
  habits.record([{ kind: "key", name: "M-d", n: 2 }], NOW - 10 * DAY);
  habits.record([{ kind: "key", name: "M-d", n: 3 }, { kind: "view", name: "Calendar", n: 1 }], NOW);
  habits.record([{ kind: "key", name: "M-d", n: 1 }], NOW);
  habits.record([{ kind: "key", name: "bad\tname", n: 1 }, { kind: "key", name: "M-x", n: 0 }], NOW);
  const week = habits.tally(7, NOW);
  assert.equal(week.counts.get("key\tM-d"), 4);
  assert.equal(week.counts.get("view\tCalendar"), 1);
  assert.equal(week.counts.size, 2, "a name with a tab, or a count of 0, is left out");
  assert.equal(week.tracked, 11, "counted since the first day kept");
  assert.equal(habits.tally(30, NOW).counts.get("key\tM-d"), 6);

  habits.record([{ kind: "key", name: "M-d", n: 1 }], NOW - 90 * DAY);
  habits.tally(7, NOW);
  assert.equal(fs.readdirSync(path.join(tmp, "habits")).length, 2, "a day over 60 days old is removed");
  fs.rmSync(path.join(tmp, "habits"), { recursive: true });
});

test("a command picked in M-p three times in a fortnight suggests its key, and each suggestion can be put off or away", () => {
  const tally = (counts, tracked = 40) => ({ counts: new Map(Object.entries(counts)), tracked });
  const views = Object.fromEntries(habits.VIEWS.map((v) => [`view\t${v.title}`, 1]));
  const args = { month: tally(views), fortnight: tally({ "palette\tM-d": 3, "palette\tM-a": 2, "palette\t@palette W": 9 }), week: tally({}), value: on, notes, dismissed: {}, now: NOW };
  const [key, ...rest] = habits.suggestionsFrom(args);
  assert.deepEqual(rest, [], "M-a picked twice and a palette-only command suggest nothing");
  assert.equal(key.id, "key:M-d");
  assert.equal(key.title, "M-d: Open the branch's diff");
  assert.match(key.body, /picked this from M-p 3 times/);
  assert.equal(key.setting, undefined, "a key changes no setting");

  assert.deepEqual(habits.suggestionsFrom({ ...args, dismissed: { "key:M-d": 0 } }), [], "put away for good");
  assert.deepEqual(habits.suggestionsFrom({ ...args, dismissed: { "key:M-d": NOW + DAY } }), [], "put off");
  assert.equal(habits.suggestionsFrom({ ...args, dismissed: { "key:M-d": NOW - 1 } }).length, 1, "back once put off long enough");

  habits.dismissSuggestion("key:M-d", false, NOW);
  habits.dismissSuggestion("off:usage", true, NOW);
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(tmp, "suggestions.json"), "utf8")), { "key:M-d": NOW + 14 * DAY, "off:usage": 0 });
});

test("a view not opened in 30 days is suggested off, only once 30 days are counted and while it and its service are on", () => {
  const tally = (counts, tracked) => ({ counts: new Map(Object.entries(counts)), tracked });
  const opened = Object.fromEntries(habits.VIEWS.filter((v) => !["Calendar", "PRs"].includes(v.title)).map((v) => [`view\t${v.title}`, 2]));
  const args = (tracked, value = on) => ({ month: tally(opened, tracked), fortnight: tally({}, tracked), week: tally({}, tracked), value, notes, dismissed: {}, now: NOW });
  assert.deepEqual(habits.suggestionsFrom(args(29)), [], "not before 30 days are counted");
  const off = habits.suggestionsFrom(args(30));
  assert.deepEqual(off.map((s) => s.id), ["off:view_calendar", "off:pr_board"]);
  assert.deepEqual(off[0].setting, { key: "view_calendar", value: false, label: "Turn it off" });
  assert.doesNotMatch(off[0].body, /looks up/, "the Calendar's feed carries on with its view off");
  assert.match(off[1].body, /stops what it looks up in the background/);
  assert.deepEqual(habits.suggestionsFrom(args(30, (key) => key !== "github")).map((s) => s.id), ["off:view_calendar"], "the PRs view isn't shown with GitHub off");
  assert.deepEqual(habits.suggestionsFrom(args(30, (key) => key !== "view_calendar")).map((s) => s.id), ["off:pr_board"]);
});

test("a feature that is off is suggested once what it helps with is done often in a week", () => {
  const tally = (counts) => ({ counts: new Map(Object.entries(counts)), tracked: 3 });
  const value = (key) => !["todo_note", "second_status"].includes(key);
  const args = (week) => ({ month: tally({}), fortnight: tally({}), week: tally(week), value, notes: {}, dismissed: {}, now: NOW });
  assert.deepEqual(habits.suggestionsFrom(args({ "key\tM-N": 9 })), []);
  // M-N picked in M-p is pressed by it too, so 6 presses and 4 picks are 6.
  assert.deepEqual(habits.suggestionsFrom(args({ "key\tM-N": 6, "palette\tM-N": 4 })), []);
  const [todo] = habits.suggestionsFrom(args({ "key\tM-N": 6, "key\tM-e": 3, "palette\t@palette N": 1 }));
  assert.equal(todo.id, "on:todo_note");
  assert.deepEqual(todo.setting, { key: "todo_note", value: true, label: "Turn it on" });
  assert.match(todo.body, /went to Notes 10 times/);
  assert.match(todo.body, /sends your notes' unticked items to Claude/, "says what it sends");
  assert.deepEqual(habits.suggestionsFrom({ ...args({ "key\tM-v": 10 }), value: on }), [], "not once it is on");
  assert.equal(habits.suggestionsFrom(args({ "key\tM-v": 10 }))[0].id, "on:second_status");
});

test("forget removes the days counted and what was put off or away", () => {
  habits.record([{ kind: "key", name: "M-d", n: 1 }], NOW);
  habits.dismissSuggestion("key:M-d", true, NOW);
  habits.forget();
  assert.equal(fs.existsSync(path.join(tmp, "habits")), false);
  assert.equal(fs.existsSync(path.join(tmp, "suggestions.json")), false);
  assert.equal(habits.tally(30, NOW).tracked, 0);
});
