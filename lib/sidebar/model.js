// Part of bin/tmux-sidebar; see the comment at the top of that file.

const { MARK } = require("./config");
const fs = require("node:fs");
const { sessionsByPane, transcriptFor, backgroundWork } = require("./claude");
const { isOn } = require("../settings");

// When an idle tab last did anything, in seconds: a Claude tab's last word in
// its conversation, else the last output to its pane's terminal, which the
// terminal device's modified time records. tmux's own window_activity can't
// say, since the sidebar in the window draws to it all the time.
const idleSince = (claude, tty) => {
  const file = claude && transcriptFor(claude);
  try {
    return Math.floor(fs.statSync(file ?? tty).mtimeMs / 1000);
  } catch {
    return undefined;
  }
};

// Most in need of you first: a Claude session waiting on you, then one that
// finished while you were elsewhere, then one working, then the rest. The same
// order agent-state.sh rolls a window's panes up into @agent by. A Claude tab
// whose turn ended with background tasks still running (a review loop, a CI
// watch) is "background": Claude carries on when they end, so it is working,
// not waiting for you.
const RANK = { waiting: 0, done: 1, working: 2, background: 2, idle: 3 };

// One tab per window, from a tmux snapshot. Sidebar panes are left out of
// everything but `sidebar`, the window's own.
// `failed(tab, err)` hears about a tab whose Claude session or transcript
// couldn't be read: that tab is shown without what it would have added, and
// every other tab as usual, so one bad file never stops the refresh, which
// is also what gives new windows their sidebar.
const tabsFrom = (snap, failed = () => {}) => {
  const claude = sessionsByPane();
  const windows = new Map();
  snap.panes.forEach((p) => {
    if (!windows.has(p.window_id)) {
      // window-names.sh keeps the whole label, PR number in front, in
      // @tab_label, and the project's accent color in the tab's format.
      const label = p["@tab_label"] || p.window_name;
      const pr = /^#(\d+) /.exec(label)?.[1];
      windows.set(p.window_id, {
        id: p.window_id,
        index: Number(p.window_index),
        session: p.session_id,
        sessionName: p.session_name,
        attached: p.session_attached !== "0",
        current: p.window_active === "1",
        width: Number(p.window_width),
        zoomed: p.window_zoomed_flag === "1",
        name: pr ? label.slice(pr.length + 2) : label,
        // The name given it by hand or by a skill, which the closed tab
        // takes back when it reopens.
        tabName: p["@tab_name"] || undefined,
        pr,
        color: /fg=#([0-9a-fA-F]{6})/.exec(p["window-status-format"])?.[1],
        state: p["@agent"] || "idle",
        // Pinned by hand (M-f) as needing you, until unpinned.
        pinned: p["@pinned"] === "1",
        // A reminder set on it (tmux/remind.sh): when, in seconds, and what for.
        remindAt: Number(p["@remind_at"]) || undefined,
        remindNote: p["@remind_note"] || undefined,
        // The reminder the daemon has said came due, by its time.
        remindSaid: Number(p["@remind_said"]) || undefined,
        // A window holding a pane while a tool stands in for it (tmux/pane-in-place.sh).
        parked: p["@pane_parked"] === "1",
        // The Notifications view's window (noticesWindow in the daemon), no tab.
        noticesWindow: p["@notices_window"] === "1",
        panes: [],
        sidebars: [],
      });
    }
    const tab = windows.get(p.window_id);
    if (p[MARK] === "1" || p.pane_title === "tmux-sidebar") tab.sidebars.push(p);
    else tab.panes.push(p);
  });
  const tabs = [...windows.values()].map((tab) => {
    const active = tab.panes.find((p) => p.pane_active === "1") ?? tab.panes[0];
    // The pane whose state the window shows, and since when: the earliest of
    // the panes in that state.
    const inState = tab.panes.filter((p) => p["@agent_state"] === tab.state).sort((a, b) => Number(a["@agent_since"]) - Number(b["@agent_since"]));
    const claudePane = inState[0] ?? tab.panes.find((p) => claude.has(p.pane_id));
    const session = claudePane && claude.get(claudePane.pane_id);
    const safely = (read, fallback) => {
      try {
        return read();
      } catch (err) {
        failed(tab, err);
        return fallback;
      }
    };
    // Read for a working tab too, since a turn an API error cut short leaves
    // its pane marked working (no Stop hook runs).
    const read = safely(() => (session && ["done", "idle", "working"].includes(tab.state) ? backgroundWork(transcriptFor(session), session.startedAt) : { count: 0 }), { count: 0 });
    // Its background work counts only for a tab between turns: a working one
    // stays working, whatever runs beside it.
    const work = tab.state === "working" ? { count: 0, apiError: read.apiError, apiErrorAt: read.apiErrorAt } : read;
    // A turn an API error cut short needs you: it shows as waiting, on the
    // error, until the conversation goes on.
    const stopped = work.apiError;
    return {
      ...tab,
      state: stopped ? "waiting" : work.count || work.scheduled ? "background" : tab.state,
      background: work.count,
      // The Monitor watches running, by description (see claude.js).
      monitors: work.monitors ?? [],
      // The loop running, { kind, round }, when the work is one (see claude.js).
      loop: work.loop ?? (work.scheduled && { kind: "scheduled", prompt: work.scheduled.prompt, at: work.scheduled.at }),
      // What Claude is doing, and what it waits on, as agent-state.sh records
      // them from its hooks.
      activity: claudePane?.["@agent_activity"] || undefined,
      wait: stopped || claudePane?.["@agent_wait"] || undefined,
      // The loop the session is running, from when its skill was asked for.
      loopSkill: claudePane?.["@agent_loop"] || undefined,
      // The loop's step, when it says (queue-and-clean's landing).
      phase: claudePane?.["@agent_phase"] || undefined,
      // Waiting on an API error since it came, in seconds as @agent_since is.
      since: stopped ? Math.floor(work.apiErrorAt / 1000) || undefined : tab.state === "idle" ? safely(() => idleSince(session, active?.pane_tty), undefined) : Number(inState[0]?.["@agent_since"]) || undefined,
      path: active?.pane_current_path,
      command: active?.pane_current_command,
      claudePane: claudePane?.pane_id,
      claude: session,
      sidebar: tab.sidebars[0],
      // The one Notes, brought beside this tab's pane with M-e
      // (tmux/notes-open.sh), rather than in a tab of its own.
      notesHere: tab.panes.length > 1 && tab.panes.some((p) => p["@notes_pane"] === "1"),
      // The Notes tab while Notes is beside another tab's pane: the pane
      // standing in for it there (tmux/notes-away.sh).
      notesAway: tab.panes.some((p) => p["@notes_placeholder"] === "1"),
      sidebarActive: tab.sidebars.some((p) => p.pane_active === "1"),
    };
  });
  // Which tab has Notes, for the Notes tab's line.
  const owner = tabs.find((t) => t.notesHere);
  return tabs.map((t) => (t.notesAway ? { ...t, notesOwner: owner && { index: owner.index, name: owner.name } } : t));
};

const byNeed = (a, b) =>
  RANK[a.state] - RANK[b.state] ||
  (a.state === "waiting" || a.state === "done" ? (a.since ?? 0) - (b.since ?? 0) : 0) ||
  a.index - b.index;

// Grouped for drawing: [{ title, tabs }], in this order, empty groups left
// out. A tab is in one group: Needs you whenever Claude waits on you or has
// finished there; otherwise In merge queue or Merged when its PR is (see
// prs.js, which sets tab.merge), even while Claude works in it; otherwise
// Build failing when a check on its open PR failed (prs.js's tab.checks,
// asked every minute), even while Claude works in it; otherwise Loops between
// a /loop's ticks; otherwise by its state. The queue is in queue order.
// A tab holding or waiting for the landing lock (landing.js sets
// tab.landing) is under Landing, in the landing queue's order, whatever
// Claude does there short of needing you.
// In Needs you, the tabs you pinned come first: you marked them yourself.
// A reminder that has come due needs you as a pin does, until you go to the
// tab; one still to come puts the tab under Reminders with the time left
// while it is idle or working (reminders setting).
const reminderDue = (tab) => Boolean(tab.remindAt) && tab.remindAt * 1000 <= Date.now() && isOn("reminders");
const reminderLater = (tab) => Boolean(tab.remindAt) && tab.remindAt * 1000 > Date.now() && isOn("reminders");
// A tab waiting on you or finished needs you whatever its reminder, which
// only puts off a tab with nothing to ask of you yet.
const needsTab = (tab) => tab.pinned || reminderDue(tab) || tab.state === "waiting" || tab.state === "done";
const pinnedFirst = (a, b) => Number(Boolean(b.pinned || reminderDue(b))) - Number(Boolean(a.pinned || reminderDue(a))) || byNeed(a, b);
const GROUPS = [
  { key: "needs", title: "Needs you", order: pinnedFirst },
  { key: "failing", title: "Build failing" },
  { key: "working", title: "Working" },
  { key: "monitoring", title: "Monitoring" },
  { key: "queued", title: "In merge queue", order: (a, b) => (a.position ?? Infinity) - (b.position ?? Infinity) || byNeed(a, b) },
  // Landing on the default branch (landing.js): the landing one first, then
  // the queue in its order.
  { key: "landing", title: "Landing", order: (a, b) => a.landing.line - b.landing.line || byNeed(a, b) },
  { key: "deploy", title: "Testing Deploy" },
  { key: "merged", title: "Merged" },
  { key: "idle", title: "Idle" },
  { key: "loops", title: "Loops" },
  // Put off by you until later, so near the bottom, above Other.
  { key: "later", title: "Reminders", order: (a, b) => a.remindAt - b.remindAt },
  { key: "other", title: "Other" },
];
// A tab whose only background work is Monitor watches is watching, not
// working: Claude has nothing to do until one reports.
// A merged PR's tab still at work on it: deploy-test waiting for the deploy
// or recording its test, or Claude working under /deploy-test or the
// /queue-and-clean that follows it after the merge.
const testingDeploy = (tab) =>
  tab.merge === "merged" && (tab.loop?.kind === "deploy" || ((tab.state === "working" || tab.state === "background") && (tab.loopSkill === "deploy" || tab.loopSkill === "queue")));
const monitoringOnly = (tab) => tab.state === "background" && tab.monitors?.length > 0 && tab.monitors.length === tab.background && !tab.loop;
// A self-paced or timed /loop between its ticks (the ↻ mark) has nothing to
// do until its next one, so it is in Loops, above Other, rather than Working.
const loopingOnly = (tab) => tab.state === "background" && tab.loop?.kind === "scheduled";
// A tab with no Claude in it (the Slack relay, Notes, a shell) is Other, at
// the bottom, so Idle holds only Claude sessions with nothing to do.
const restOf = (tab) => (tab.claude ? "idle" : "other");
// A tab whose open PR, neither queued nor merged, has a check that failed
// (build_failing setting).
const buildFailing = (tab) => tab.prState === "OPEN" && Boolean(tab.checks?.fail) && !tab.merge && isOn("build_failing");
const groupOf = (tab) =>
  needsTab(tab) ? "needs" : reminderLater(tab) ? "later" : tab.landing ? "landing" : testingDeploy(tab) ? "deploy" : tab.merge === "queued" || tab.merge === "merged" ? tab.merge : buildFailing(tab) ? "failing" : monitoringOnly(tab) ? "monitoring" : loopingOnly(tab) ? "loops" : tab.state === "working" || tab.state === "background" ? "working" : restOf(tab);
// `frozen`, a layout from frozenFrom, keeps each tab it knows in the group
// and place it had then, whatever its state since; a tab opened since goes
// where its state puts it, after them.
const grouped = (tabs, frozen) =>
  GROUPS.map((group) => ({
    key: group.key,
    title: group.title,
    tabs: tabs
      .filter((t) => (frozen?.get(t.id)?.group ?? groupOf(t)) === group.key)
      .sort((a, b) => (frozen?.get(a.id)?.at ?? Infinity) - (frozen?.get(b.id)?.at ?? Infinity) || (group.order ?? byNeed)(a, b)),
  })).filter((g) => g.tabs.length);
// The tabs under Needs you, in the order the sidebar lists them.
const needsYou = (tabs) => tabs.filter((t) => groupOf(t) === "needs").sort(pinnedFirst);
// Each tab's group and place as grouped lays them out now: window ID ->
// { group, at }.
const frozenFrom = (tabs) =>
  new Map(GROUPS.flatMap((group) => tabs.filter((t) => groupOf(t) === group.key).sort(group.order ?? byNeed).map((t) => [t.id, group.key])).map(([id, group], at) => [id, { group, at }]));

// Mission control's finer stages, in this order: each loop running in the
// background (see claude.js) is a stage of its own rather than part of
// Working. A tab is in one: Needs you first, then its PR's place (In merge
// queue, Merged), then the loop it is running, then its state, so a
// queue-and-clean watching a queued PR shows the PR's place. `color` names the STATES entry whose
// color the stage's header wears.
const STAGES = [
  { key: "needs", title: "Needs you", color: "waiting", order: pinnedFirst },
  { key: "failing", title: "Build failing", color: "failing" },
  { key: "working", title: "Working", color: "working" },
  { key: "monitoring", title: "Monitoring", color: "monitor" },
  { key: "review", title: "Review loop", color: "review" },
  { key: "ci", title: "CI loop", color: "ci" },
  { key: "queue", title: "Queue-and-clean", color: "queue" },
  { key: "scheduled", title: "Loops", color: "scheduled" },
  { key: "queued", title: "In merge queue", color: "queued", order: GROUPS.find((g) => g.key === "queued").order },
  { key: "landing", title: "Landing", color: "landing", order: GROUPS.find((g) => g.key === "landing").order },
  { key: "deploy", title: "Testing Deploy", color: "deploy" },
  { key: "merged", title: "Merged", color: "merged" },
  { key: "idle", title: "Idle", color: "idle" },
  { key: "later", title: "Reminders", color: "later", order: (a, b) => a.remindAt - b.remindAt },
  { key: "other", title: "Other", color: "idle" },
];
const stageOf = (tab) => {
  if (needsTab(tab)) return "needs";
  if (reminderLater(tab)) return "later";
  if (tab.landing) return "landing";
  if (testingDeploy(tab)) return "deploy";
  if (tab.merge === "queued" || tab.merge === "merged") return tab.merge;
  // A CI loop at work is the fix for a failing build, so it keeps its own
  // stage; once it stops, the tab is Build failing again.
  const ciAtWork = (tab.state === "working" || tab.state === "background") && (tab.loop?.kind === "ci" || tab.loopSkill === "ci");
  if (buildFailing(tab) && !ciAtWork) return "failing";
  if (monitoringOnly(tab)) return "monitoring";
  if (tab.state === "background" && tab.loop) return tab.loop.kind;
  // Between a loop's rounds, Claude working in the foreground is the loop's too.
  if ((tab.state === "working" || tab.state === "background") && tab.loopSkill) return tab.loopSkill;
  return tab.state === "working" || tab.state === "background" ? "working" : restOf(tab);
};
const staged = (tabs) =>
  STAGES.map((stage) => ({ ...stage, tabs: tabs.filter((t) => stageOf(t) === stage.key).sort(stage.order ?? byNeed) })).filter((s) => s.tabs.length);

module.exports = { RANK, tabsFrom, byNeed, grouped, frozenFrom, staged, monitoringOnly, testingDeploy, buildFailing, needsYou, reminderDue, reminderLater };
