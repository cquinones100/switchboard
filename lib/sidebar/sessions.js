// Part of bin/tmux-sidebar; see the comment at the top of that file.
//
// Mission control's Sessions view: each Claude tab's session, how full its
// context is, its model, how long it has run and how long since it last did
// anything, fullest first, to tell which to compact or close. The context
// and model are what Claude Code gives the statusline, which writes them to
// STATE_DIR/context/<session> (claude/statusline-command.sh); the age is the
// session file's start, and the last activity its transcript's last write.
//
// Above them, while the usage setting is on, what Claude has cost today and
// how much of the subscription's rate limits is used, which the statusline
// alone is told too: STATE_DIR/usage/spend-<session> holds each session's
// share of a day ("<day>\t<spent>\t<cost so far>\t<pid>"), and usage/limits the
// account's rate_limits as Claude Code gives them. The cost is Claude Code's
// estimate at API prices, for interactive sessions only (claude -p has no
// statusline), and the limits are as the last session to draw saw them.
//
// Below them, with the request_audit setting on, today's requests to services
// outside this Mac, as lib/requests.js counts them: the total by protocol,
// then each service with its own protocols, busiest first.

const fs = require("node:fs");
const path = require("node:path");
const { STATE_DIR } = require("./config");
const { transcriptFor } = require("./claude");
const { ago } = require("./text");
const { tally } = require("../requests");
const { isOn } = require("../settings");

// Context this full or more needs you: compact or start over.
const FULL = 80;
// A rate limit this used or more needs you: slow down or wait for its reset.
const LIMIT_FULL = 80;

const localDay = (date = new Date()) =>
  `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;

// When a window resets: in so long today, else its weekday and time.
const resets = (at) => {
  const s = at - Math.floor(Date.now() / 1000);
  if (s <= 0) return "resets now";
  if (s < 86400) return `resets in ${Math.floor(s / 3600) ? `${Math.floor(s / 3600)}h ` : ""}${Math.floor((s % 3600) / 60)}m`;
  return `resets ${new Date(at * 1000).toLocaleString("en-US", { weekday: "short", hour: "numeric", minute: "2-digit" })}`;
};

const usageItems = () => {
  const dir = path.join(STATE_DIR, "usage");
  const today = localDay();
  let files = [];
  try {
    files = fs.readdirSync(dir).filter((f) => f.startsWith("spend-"));
  } catch {}
  const shares = files
    .map((f) => {
      try {
        return fs.readFileSync(path.join(dir, f), "utf8").split("\t");
      } catch {
        return [];
      }
    })
    .filter(([day, spent]) => day === today && Number(spent) > 0);
  const spent = shares.reduce((sum, [, s]) => sum + Number(s), 0);
  let limits = {};
  try {
    limits = JSON.parse(fs.readFileSync(path.join(dir, "limits"), "utf8"));
  } catch {}
  const now = Math.floor(Date.now() / 1000);
  const windows = [
    ["five_hour", "5-hour limit"],
    ["seven_day", "Weekly limit"],
    ["spend_limit", "Spend limit"],
  ]
    .filter(([key]) => Number.isFinite(limits[key]?.used_percentage) && limits[key].resets_at > now)
    .map(([key, name]) => ({
      title: `${name} ${Math.round(limits[key].used_percentage)}% used`,
      detail: [limits[key].used_usd !== undefined && `$${limits[key].used_usd.toFixed(2)} of $${limits[key].limit_usd}`, resets(limits[key].resets_at)].filter(Boolean).join(" · "),
      state: limits[key].used_percentage >= LIMIT_FULL ? "attention" : "idle",
      used: -1,
    }));
  return [
    { title: `Spent today $${spent.toFixed(2)}`, detail: `${shares.length} session${shares.length === 1 ? "" : "s"}, at API prices`, state: "idle", used: -1 },
    ...windows,
    ...(isOn("request_audit") ? requestItems() : []),
  ];
};

const number = (n) => n.toLocaleString("en-US");
const byCount = (counts) =>
  Object.entries(counts)
    .sort((a, b) => b[1] - a[1])
    .map(([name, n]) => `${name} ${number(n)}`)
    .join(" · ");
const requestItems = () => {
  const day = tally();
  return [
    { title: `Requests today ${number(day.total)}`, detail: day.total ? `by protocol: ${byCount(day.protocols)}` : "none yet", state: "idle", used: -1 },
    ...day.services.map((service) => ({ title: `  ${service.service} ${number(service.total)}`, detail: byCount(service.protocols), state: "idle", used: -1 })),
    { title: "  Not counted", detail: "what Claude Code sessions reach themselves (their model and connectors), skills' scripts, wt, wtb, wtpr and shape, Copilot", state: "idle", used: -1 },
  ];
};

const contextOf = (sessionId) => {
  try {
    const [used, model] = fs.readFileSync(path.join(STATE_DIR, "context", sessionId), "utf8").split("\t");
    return { used: Number(used), model };
  } catch {
    return {};
  }
};

const lastWrite = (session) => {
  try {
    return Math.floor(fs.statSync(transcriptFor(session)).mtimeMs / 1000);
  } catch {
    return undefined;
  }
};

const sessionItems = (tabs) =>
  tabs
    .filter((tab) => tab.claude?.sessionId)
    .map((tab) => {
      const { used, model } = contextOf(tab.claude.sessionId);
      const started = tab.claude.startedAt ? Math.floor(tab.claude.startedAt / 1000) : undefined;
      const quiet = tab.state === "idle" || tab.state === "done" ? lastWrite(tab.claude) : undefined;
      const facts = [
        Number.isFinite(used) ? `context ${Math.round(used)}%` : "context unknown",
        model,
        started && `up ${ago(started)}`,
        quiet && `idle ${ago(quiet)}`,
      ].filter(Boolean);
      return {
        title: `${tab.index} ${tab.name}`,
        detail: facts.join(" · "),
        state: used >= FULL ? "attention" : tab.state === "idle" || tab.state === "done" ? "idle" : "active",
        used: Number.isFinite(used) ? used : -1,
        tab,
      };
    })
    .sort((a, b) => b.used - a.used);

module.exports = { FULL, LIMIT_FULL, sessionItems, usageItems };
