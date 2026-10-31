// Part of bin/tmux-sidebar; see the comment at the top of that file.
//
// Mission control's built-in views, after its tabs and before the plugins'.
// Each is a list of items, drawn as the plugins' are, { title, detail,
// state, time }, and an item says what Enter does with it: `tab`, go there;
// `url`, open it; `dir`, open a tab there.
//
// Closed: the tabs closed most recently, to reopen (see closed.js).
// PRs: your pull requests as a board, by where they stand (see prboard.js).
// Tickets: your tickets, in Linear or Jira, as a board, by state (see ticketboard.js).
// CI: your pull requests with failing checks, and why (see ciboard.js).
// Inbox: your unread Linear mentions, comments and assignments (see inbox.js), Linear only.
// Calendar: today as Google Calendar's Day view shows it, from the day /today
// last saved (see calendar.js), once it has saved one.
// Slack: what is new for you on Slack in the last day, as /slack-updates last
// saved it (see slack.js), once it has saved a look.
// Sessions: each Claude tab's context, model, age and quiet, under today's
// spend and the rate limits' headroom (see sessions.js).
// Settings: every option in tmux/settings.json under its category, with its
// value, which Enter or Space changes (see lib/settings.js and the daemon).
//
// The work map: every piece of work in the repositories your tabs are in,
// one row each: each worktree, and a main checkout a tab is in, with its
// ticket (from the branch), the tab it is open in or none, its PR and where
// that stands, and its directory. Work with no tab open is still listed, so
// nothing in flight is forgotten. The worktrees are read with git every
// minute, in the background (refreshWorktrees), and drawing reads only that.
// Work with no tab also comes from bin/wt-revive (refreshRevived), which looks
// further, in every repository a Claude session used in the last two weeks,
// and says when each worktree was last used and what you last asked there;
// Enter on such a row opens a tab there and resumes its Claude, as M-o did
// before it opened this view.
//
// Every open tab is a row too, one in no repository included (the Slack
// relay, a loop's tab), so M-j, which lists the open tabs' rows, reaches them
// all. A tab belongs to the worktree it is in, or under, the deepest one.
// Each row is drawn in columns (cells: ticket, title, tab, PR, when last
// used), its directory and last prompt (more) shown only on the chosen row.
// Merged work no tab is in comes last, under a "Merged, can be removed"
// header that is closed until Enter opens it (see viewsFor), and x removes
// the chosen one's worktree, asking first.

const fs = require("node:fs");
const { ownItems } = require("./own-config");
const os = require("node:os");
const path = require("node:path");
const { execFile } = require("node:child_process");
const { mergeOf } = require("./prs");
const { closedItems } = require("./closed");
const { boardView } = require("./prboard");
const { ticketView } = require("./ticketboard");
const { ciView } = require("./ciboard");
const { inboxView, inboxOn } = require("./inbox");
const { calendarView } = require("./calendar");
const { slackView } = require("./slack");
const { sessionItems, usageItems } = require("./sessions");
const { currentSettings, reloadSettings, isOn, valueWords } = require("../settings");

const home = os.homedir();
const tilde = (dir) => (dir && dir.startsWith(home) ? `~${dir.slice(home.length)}` : dir ?? "");

const git = (dir, args) =>
  new Promise((resolve) => execFile("git", ["-C", dir, ...args], { encoding: "utf8", timeout: 10_000 }, (err, stdout) => resolve(err ? undefined : stdout)));

// A directory's main checkout, asked once per directory.
const mains = new Map();
const mainOf = async (dir) => {
  if (!dir) return undefined;
  if (!mains.has(dir)) {
    const common = (await git(dir, ["rev-parse", "--path-format=absolute", "--git-common-dir"]))?.trim();
    mains.set(dir, common ? common.replace(/\/\.git\/?$/, "") : undefined);
  }
  return mains.get(dir);
};

// main checkout -> [{ path, branch }], as last read.
const worktrees = new Map();
const refreshWorktrees = async (tabs) => {
  const repos = [...new Set((await Promise.all(tabs.map((t) => mainOf(t.path)))).filter(Boolean))];
  await Promise.all(
    repos.map(async (repo) => {
      const out = await git(repo, ["worktree", "list", "--porcelain"]);
      if (out === undefined) return;
      worktrees.set(
        repo,
        out.split("\n\n").filter(Boolean).map((entry) => ({
          path: /^worktree (.*)$/m.exec(entry)?.[1],
          branch: /^branch refs\/heads\/(.*)$/m.exec(entry)?.[1],
          main: entry.startsWith(`worktree ${repo}\n`),
        })).filter((w) => w.path),
      );
    }),
  );
};

// wt-revive's rows by worktree: { tree, repo, branch, when, age, prompt }.
const revived = new Map();
const WT_REVIVE = path.join(__dirname, "../../bin/wt-revive");
const refreshRevived = () =>
  new Promise((resolve) =>
    execFile(process.execPath, [WT_REVIVE, "--json", home], { encoding: "utf8", timeout: 30_000 }, (err, stdout) => {
      if (!err) {
        revived.clear();
        stdout.split("\n").filter(Boolean).forEach((line) => {
          try {
            const row = JSON.parse(line);
            revived.set(row.tree, row);
          } catch {}
        });
      }
      resolve();
    }),
  );

// Every worktree the map lists: git's, then wt-revive's in other repositories.
const allWorktrees = () => {
  const listed = [...worktrees.values()].flat();
  const known = new Set(listed.map((w) => w.path));
  return [...listed, ...[...revived.values()].filter((r) => !known.has(r.tree)).map((r) => ({ path: r.tree, branch: r.branch, main: false }))];
};

// The worktrees no tab is in, as stand-ins for tabs, so their PRs are asked
// about by branch along with the tabs' (see prs.js).
const untabbed = (tabs) =>
  allWorktrees().filter((w) => !w.main && w.branch && !tabs.some((t) => t.path === w.path)).map((w) => ({ path: w.path, branch: w.branch }));

const TICKET = /^([a-z][a-z0-9]*-\d+)/i;
// Nothing at all with the github setting off.
const prWords = (facts) => {
  if (!isOn("github")) return "";
  if (!facts.pr) return "no PR";
  const c = facts.checks;
  const where = facts.merge === "merged" ? "merged" : facts.merge === "queued" ? `in merge queue${facts.position ? ` at ${facts.position}` : ""}` : c?.fail ? `✗${c.fail} failing` : c?.pending ? "checks running" : "open";
  return `#${facts.pr} ${where}`;
};
// Needing you first, then work going on, open PRs, merged ones, and work no
// tab is in last.
const rank = (row) => (row.state === "attention" ? 0 : row.state === "active" ? 1 : row.merge === "merged" ? 3 : row.tab ? 2 : 4);
// Work with no tab, most recently used first.
const byRank = (a, b) => rank(a) - rank(b) || (rank(a) === 4 ? (b.when ?? 0) - (a.when ?? 0) : 0);

// What a row is called: its tab's name, or its branch in words, without the
// ticket the row already leads with.
const named = (tab, w, ticket) => {
  const name = tab?.name ?? (w.branch ?? w.path.split("/").pop()).replace(TICKET, "").replace(/^[-_]+/, "").replace(/[-_]+/g, " ");
  return ticket ? name.replace(new RegExp(`^${ticket}:?\\s*`, "i"), "") : name;
};

const stateOf = (tab, facts) => {
  const needs = tab && (tab.state === "waiting" || tab.state === "done" || tab.pinned);
  return needs || facts.checks?.fail ? "attention" : tab && tab.state !== "idle" ? "active" : "idle";
};

const workMap = (tabs) => {
  const all = allWorktrees();
  // The worktree a tab is in or under, the deepest: a worktree sits under its
  // main checkout's directory, in .claude/worktrees.
  const ownerOf = (t) => all.filter((w) => t.path && (t.path === w.path || t.path.startsWith(`${w.path}/`))).sort((a, b) => b.path.length - a.path.length)[0];
  const open = tabs.filter((t) => !t.parked);
  const rows = [];
  all.forEach((w) => {
    const tab = open.find((t) => ownerOf(t) === w);
    if (w.main && !tab) return;
    const facts = tab ?? { ...mergeOf({ path: w.path, branch: w.branch }) };
    // No ticket column with the tracker setting off.
    const ticket = isOn("tracker") ? TICKET.exec(w.branch ?? "")?.[1]?.toUpperCase() : undefined;
    const last = !tab && revived.get(w.path);
    const title = named(tab, w, ticket);
    const where = tab ? `tab ${tab.index}` : "no tab";
    rows.push({
      title: `${ticket ? `${ticket}  ` : ""}${title}`,
      detail: [where, prWords(facts), last && `used ${last.age}`, tilde(w.path), last?.prompt].filter(Boolean).join(" · "),
      cells: [ticket ?? "", title, where, prWords(facts), last ? `used ${last.age}` : ""],
      more: [tilde(w.path), last?.prompt].filter(Boolean).join(" · "),
      state: stateOf(tab, facts),
      merge: facts.merge,
      merged: !tab && facts.merge === "merged",
      tab,
      dir: tab ? undefined : w.path,
      when: last?.when,
    });
  });
  // The open tabs no worktree row took: in no repository, or a second tab
  // in the same worktree.
  open.filter((t) => !rows.some((r) => r.tab === t)).forEach((tab) => {
    const where = `tab ${tab.index}`;
    rows.push({
      title: tab.name,
      detail: [where, tab.pr ? prWords(tab) : "", tilde(tab.path)].filter(Boolean).join(" · "),
      cells: ["", tab.name, where, tab.pr ? prWords(tab) : "", ""],
      more: tilde(tab.path),
      state: stateOf(tab, tab),
      merge: tab.merge,
      merged: false,
      tab,
    });
  });
  const merged = rows.filter((r) => r.merged);
  const header = merged.length ? [{ title: "Merged, can be removed", group: "merged", count: merged.length, state: "idle", detail: "" }] : [];
  return [...rows.filter((r) => !r.merged).sort(byRank), ...header, ...merged.sort(byRank)];
};

// Each category a header, as the Work map's merged work has, then its
// options: the name, the value, and what it does. An option at "off" is drawn
// as idle. `search` is what a / filter matches besides the title and detail.
// Where your CLAUDE.md's own rules come from (setup.sh's claude_md):
// ~/.config/switchboard/claude.json's repo or path, else the local file.
const CLAUDE_JSON = () => path.join(os.homedir(), ".config/switchboard/claude.json");
const LOCAL_CLAUDE_MD = "~/.config/switchboard/CLAUDE.md";
const claudeMdSource = () => {
  let config = {};
  try {
    config = JSON.parse(fs.readFileSync(CLAUDE_JSON(), "utf8"));
  } catch {}
  if (config.repo) return { kind: "repo", words: `repo: ${config.repo}` };
  if (config.path) return { kind: "path", words: config.path };
  return { kind: "local", words: fs.existsSync(path.join(os.homedir(), ".config/switchboard/CLAUDE.md")) ? LOCAL_CLAUDE_MD : "none yet" };
};
const claudeMdItems = () => {
  const source = claudeMdSource();
  return [
    { title: "Claude Code", group: "category", count: 1, state: "idle", detail: "" },
    { title: "Your CLAUDE.md", detail: `Your own rules, read with Switchboard's part: ${source.words}`, value: { repo: "repository", path: "a file", local: "local file" }[source.kind], setting: "claude_md", claudeMd: source.kind, state: "active", search: `Claude Code CLAUDE.md rules ${source.words}` },
  ];
};

const settingsItems = () => [...settingItemsOf(), ...claudeMdItems(), ...ownItems()];
const settingItemsOf = () =>
  [...new Set(currentSettings().map((o) => o.category))].flatMap((category) => {
    const members = currentSettings().filter((o) => o.category === category);
    return [
      { title: category, group: "category", count: members.length, state: "idle", detail: "" },
      ...members.map((o) => ({
        title: o.label,
        detail: o.description,
        value: valueWords(o),
        setting: o.key,
        state: o.value === false || o.value === "off" ? "idle" : "active",
        search: `${category} ${o.key} ${valueWords(o)}`,
      })),
    ];
  });

// The built-in views, as mission control draws them, each but Settings only
// while its setting is on (Mission control in Settings): the PRs view needs
// the github setting too, the Tickets view the tracker one, and the Inbox
// Linear as the tracker. The Work map is there whatever its setting for
// `workMap`, M-j's and M-o's pickers, which are it.
const builtInViews = (state, { workMap: picker = false } = {}) => [
  ...(isOn("view_work_map") || picker ? [{ title: "Work map", items: workMap(state.tabs), loaded: true, builtIn: true }] : []),
  ...(isOn("view_calendar") ? [calendarView(state.tabs)].filter(Boolean) : []),
  ...(isOn("view_slack") ? [slackView(state.tabs)].filter(Boolean) : []),
  ...(isOn("view_closed") ? [{ title: "Closed", items: closedItems(state.tabs), loaded: true, builtIn: true }] : []),
  ...(isOn("github") && isOn("pr_board") ? [boardView()] : []),
  ...(isOn("github") && isOn("view_ci") ? [ciView()] : []),
  ...(isOn("tracker") && isOn("ticket_board") ? [ticketView()] : []),
  ...(inboxOn() ? [inboxView()] : []),
  ...(isOn("view_sessions") ? [{ title: "Sessions", items: sessionItems(state.tabs), loaded: true, builtIn: true }] : []),
  // Today's Claude spend and the rate limits' headroom, a view of its own.
  ...(isOn("usage") ? [{ title: "Usage", items: usageItems(), loaded: true, builtIn: true, empty: "No Claude session has reported its usage yet today: each one does as its statusline draws." }] : []),
  { title: "Settings", items: settingsItems(), loaded: true, builtIn: true, settings: true },
];

module.exports = { CLAUDE_JSON, claudeMdSource, builtInViews, reloadSettings, currentSettings, refreshWorktrees, refreshRevived, untabbed, workMap, worktrees, revived, mainOf };
