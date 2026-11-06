// Tests for lib/habits-report.js, M-p's "How you use Switchboard".
// Run with: node --test test/*.test.js

const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

process.env.TMUX_SIDEBAR_STATE_DIR = fs.mkdtempSync(path.join(os.tmpdir(), "habits-report-"));
const { reportLines } = require("../lib/habits-report");
const plain = (line) => line.replace(/\x1b\[[0-9;]*m/g, "");

const tally = (counts, tracked) => ({ counts: new Map(Object.entries(counts)), tracked });

test("the report lists the keys pressed most, the M-p picks, the views opened and the suggestions", () => {
  const month = tally({ "key\tM-d": 40, "key\tM-a": 12, "key\tM-3": 5, "palette\tM-d": 4, "palette\t@palette W": 2, "view\tCalendar": 3, "view\tSettings": 5, "view\tCI": 1 }, 12);
  const week = tally({ "key\tM-d": 9 }, 12);
  const lines = reportLines({
    month,
    week,
    value: (key) => key !== "github",
    notes: { "M-d": "Open the branch's diff #git", "M-a": "Go to the next tab #tabs", "M-1": "Go to a tab by number (M-1 to M-9) #tabs" },
    paletteNotes: { "@palette W": "Start work on a note's items (wtn) #notes" },
    suggestions: [{ title: "M-d: Open the branch's diff" }],
  }).map(plain);
  const text = lines.join("\n");
  assert.match(text, /last 12 days/);
  assert.match(text, /M-d +9 +40 +Open the branch's diff$/m, "the week's and month's presses, the tags left out");
  assert.ok(text.indexOf("M-d ") < text.indexOf("M-a "), "most pressed first");
  assert.match(text, /M-3 .*Go to a tab by number/, "M-2 to M-9 take M-1's description");
  assert.match(text, /4 +Open the branch's diff {2}· M-d does it/);
  assert.match(text, /2 +Start work on a note's items \(wtn\)$/m);
  assert.match(text, /3 +Calendar$/m);
  assert.match(text, /0 +Slack {2}· not opened/);
  assert.match(text, /5 +Settings\n +3 +Calendar\n +1 +CI\n/, "every view counted, most opened first, one now off too");
  assert.doesNotMatch(text, /PRs|Merge queues/, "views of a service turned off aren't listed as not opened");
  assert.match(text, /Suggestions now\n +M-d: Open the branch's diff/);
});

test("the report's period stops at the 30 days it counts", () => {
  const lines = reportLines({ month: tally({}, 45), week: tally({}, 45), value: () => true, notes: {}, paletteNotes: {}, suggestions: [] }).map(plain);
  assert.match(lines.join("\n"), /for the last 30 days\./);
});

test("with nothing counted yet, the report says so", () => {
  const lines = reportLines({ month: tally({}, 0), week: tally({}, 0), value: () => true, notes: {}, paletteNotes: {}, suggestions: [] }).map(plain);
  assert.match(lines.join("\n"), /Nothing counted yet/);
});
