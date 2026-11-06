#!/usr/bin/env node
// How you use Switchboard, from what the habits setting has counted
// (lib/sidebar/habits.js), in a popup: M-p's "How you use Switchboard"
// (tmux/habits-report.sh). It reads the same counts the suggestions do, and
// nothing else: the keys pressed this week and month, the commands picked in
// M-p, mission control's views opened, and the suggestions waiting now.
// j and k (the arrows, the wheel) scroll; q, Esc, Enter or a click closes,
// and in easy mode Tab reaches the box's ×, Space or Enter pressing it.
//
// reportLines builds the lines from the counts alone, so its test needs no
// tmux or terminal.

const { execFileSync } = require("node:child_process");
const habits = require("./sidebar/habits");
const { currentSettings } = require("./settings");
const { fg, pad, BOLD, RESET, theme } = require("./sidebar/text");

const TOP = 10;
const tags = (note) => String(note ?? "").replace(/\s+#.*$/, "");

// `month` and `week` habits.tally's, `value(key)` a setting's value, `notes`
// the root keys' descriptions and `paletteNotes` M-p's own commands' (by the
// key palette.sh presses, "@palette W"), `suggestions` those waiting now.
const reportLines = ({ month, week, value, notes, paletteNotes, suggestions }) => {
  const p = theme();
  const head = (text) => `${BOLD}${fg(p.bright)}${text}${RESET}`;
  const dim = (text) => `${fg(p.dim)}${text}${RESET}`;
  const of = (t, kind) => [...t.counts].filter(([k]) => k.startsWith(`${kind}\t`)).map(([k, n]) => [k.slice(kind.length + 1), n]);
  if (!month.tracked) return ["", " Nothing counted yet: counting starts as you use the workspace.", ""];
  const days = Math.min(month.tracked, 30);
  const lines = ["", ` Counted on this Mac only, for the last ${days} day${days === 1 ? "" : "s"}${days < 30 ? " of up to 30" : ""}.`, ""];

  const keyWeek = new Map(of(week, "key"));
  const keys = of(month, "key").sort((a, b) => b[1] - a[1]).slice(0, TOP);
  lines.push(head(" Keys you press most") + dim(" · this week, last 30 days"));
  if (!keys.length) lines.push(dim("   none yet"));
  keys.forEach(([key, n]) => lines.push(`   ${pad(key, 10)}${pad(String(keyWeek.get(key) ?? 0).padStart(5), 7)}${pad(String(n).padStart(6), 8)}${tags(notes[key] ?? (/^M-[2-9]$/.test(key) ? notes["M-1"] : ""))}`));
  lines.push("");

  const picks = of(month, "palette").sort((a, b) => b[1] - a[1]).slice(0, TOP);
  lines.push(head(" Picked in M-p") + dim(" · last 30 days"));
  if (!picks.length) lines.push(dim("   none yet"));
  picks.forEach(([key, n]) => {
    const own = key in notes;
    lines.push(`   ${pad(String(n).padStart(5), 8)}${tags(own ? notes[key] : paletteNotes[key] ?? key)}${own ? dim(`  · ${key} does it`) : ""}`);
  });
  lines.push("");

  const opened = new Map(of(month, "view"));
  lines.push(head(" Mission control's views opened") + dim(" · last 30 days"));
  // Every view counted, most opened first (Tabs and Settings, and one whose
  // setting is now off, too), then the views that are on and weren't.
  const unopened = habits.VIEWS.filter((v) => value(v.key) !== false && (!v.needs || value(v.needs) !== false) && !opened.has(v.title));
  [...opened].sort((a, b) => b[1] - a[1]).forEach(([title, n]) => lines.push(`   ${pad(String(n).padStart(5), 8)}${title}`));
  unopened.forEach((v) => lines.push(`   ${pad("0".padStart(5), 8)}${v.title}${dim("  · not opened")}`));
  lines.push("");

  lines.push(head(" Suggestions now"));
  if (!suggestions.length) lines.push(dim("   none"));
  suggestions.forEach((s) => lines.push(`   ${s.title}`));
  lines.push("", dim(" Each suggestion is in the sidebar's notifications, where Enter acts on it."));
  return lines;
};

const describe = (table) => {
  try {
    return Object.fromEntries(
      execFileSync("tmux", ["list-keys", "-T", table, "-F", "#{key_string}\t#{key_note}"], { encoding: "utf8", env: { ...process.env, LC_ALL: "en_US.UTF-8" } })
        .split("\n")
        .map((line) => line.split("\t"))
        .filter(([key, note]) => key && note)
        .map(([key, note]) => [table === "palette" ? `@palette ${key}` : key, note]),
    );
  } catch {
    return {};
  }
};

const main = () => {
  const { framed, closable } = require("./sidebar/render");
  const value = (key) => currentSettings().find((o) => o.key === key)?.value;
  const notes = describe("root");
  const lines =
    value("habits") === false
      ? ["", " Counting is off: Settings' \"Suggestions from how you work\" turns it on.", ""]
      : reportLines({ month: habits.tally(30), week: habits.tally(7), value, notes, paletteNotes: describe("palette"), suggestions: habits.suggestions({ value, notes }) });
  let top = 0;
  // In easy mode the box has a × (closable), the one thing in it a click
  // presses, so Tab and Shift-Tab go to it and Space or Enter press it.
  let closeFocus = false;
  const draw = () => {
    const cols = process.stdout.columns || 80;
    const rows = process.stdout.rows || 24;
    top = Math.max(0, Math.min(top, lines.length - (rows - 4)));
    const box = framed({ title: "How you use Switchboard", body: lines.slice(top), keys: "j k scroll · q closes", closeFocus }, cols, rows);
    process.stdout.write(`\x1b[H\x1b[2J${box.join("\r\n")}`);
  };
  const done = () => {
    process.stdout.write("\x1b[?1000l\x1b[?1006l\x1b[?25h");
    process.exit(0);
  };
  process.stdout.write("\x1b[?25l\x1b[?1000h\x1b[?1006h");
  process.stdin.setRawMode?.(true);
  process.stdin.on("data", (data) => {
    const key = data.toString();
    const wheel = /^\x1b\[<(6[45]);/.exec(key);
    if ((key === "\t" || key === "\x1b[Z") && closable()) closeFocus = !closeFocus;
    else if (key === " " && closeFocus) return done();
    else if (key === "j" || key === "\x1b[B" || wheel?.[1] === "65") top++;
    else if (key === "k" || key === "\x1b[A" || wheel?.[1] === "64") top--;
    else if (/^\x1b\[<0;\d+;\d+M$/.test(key) || ["q", "\x1b", "\r", "\x03"].includes(key)) return done();
    else return;
    draw();
  });
  process.stdin.on("end", done);
  process.stdout.on("resize", draw);
  draw();
};

if (require.main === module) main();

module.exports = { reportLines };
