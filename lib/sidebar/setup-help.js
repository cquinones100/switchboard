// Part of bin/tmux-sidebar; see the comment at the top of that file.
//
// What a mission control view says when what it reads from isn't set up yet:
// what is missing and how to set it up, so a view that is on is always there
// to go to and never just empty, "Loading…" or a bare error. Each view takes
// its words from here (and the Slack view from lib/connector.js, whose
// message names /mcp), so they read alike.

const { execFile } = require("node:child_process");

// The tracker's own words for what it needs (lib/tracker), and when it is
// read again.
const trackerSetup = (provider) => `${provider.missing} Mission control picks it up within two minutes.`;

const CALENDAR =
  "The calendar isn't set up. Either: (1) give mission control your Google Calendar's secret address: in Google Calendar, open Settings, your calendar, Integrate calendar, copy \"Secret address in iCal format\", then run security add-generic-password -U -s google-calendar-ical -a \"$USER\" -w in a terminal and paste it at its prompt; mission control reads it every five minutes. Or (2) connect Google Calendar in Claude (/mcp in Claude Code) and open a Calendar tab (M-p, \"Open a Calendar tab\"), which runs /today every 30 minutes.";

const NO_REPOS = "None of your tabs is in a GitHub repository yet: open a tab in one (a clone with an origin on github.com) and its pull requests show here.";

// Why gh couldn't answer, and what to do: not installed, not signed in, or
// what it said.
const githubWhy = (err) => {
  const text = String(err?.message ?? err ?? "");
  if (err?.code === "ENOENT" || /command not found|ENOENT/.test(text)) return "GitHub isn't set up: this needs GitHub's CLI. Run brew install gh, then gh auth login, in a terminal.";
  if (/auth login|not logged|authenticat|HTTP 401|bad credentials|token/i.test(text)) return "GitHub isn't signed in: run gh auth login in a terminal, and choose GitHub.com.";
  return `Couldn't ask GitHub (gh): ${text.split("\n").find((line) => line.trim()) ?? "no answer"}`;
};

// Whether gh is there and signed in, for a view whose fetch failed without
// saying why. Answers githubWhy's words, or undefined when all is well.
const githubCheck = () =>
  new Promise((resolve) =>
    execFile("gh", ["auth", "status"], { encoding: "utf8", timeout: 10_000 }, (err, stdout, stderr) => resolve(err ? githubWhy(err.code === "ENOENT" ? err : { message: `${stderr}${stdout}` || err.message }) : undefined)),
  );

module.exports = { trackerSetup, CALENDAR, NO_REPOS, githubWhy, githubCheck };
