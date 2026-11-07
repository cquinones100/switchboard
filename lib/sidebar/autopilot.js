// Part of bin/tmux-sidebar; see the comment at the top of that file.
//
// Working a note's checklist continuously: with the mode on (M-p's "Work the
// note continuously"), the daemon keeps `slots` tabs working through a note's
// items, each opened with wtn (tmux/wtn-open.zsh next), and opens the next
// whenever one lands. A tab waiting on you frees its slot but still counts
// toward `cap`, the most of the mode's tabs open at once. Its state, what
// you chose, is kept in STATE_DIR/autopilot.json:
// { on, note, repo, slots, cap, paused, readyOnly, skipped, spendLimit, said }.
// Each tab it opens is marked @autopilot with the time it opened, and
// @autopilot_in once seen in its worktree, so it counts only its own tabs.
// Once queue-and-clean has landed one and moved it back to the main clone,
// it is marked @autopilot_left with the time, @autopilot_seen once you have
// had it in front (here, or agent-state.sh seen), and closed once you leave
// it, or CLOSE_AFTER_S after landing if you never look; a prompt typed in it
// since makes it yours, out of the mode (the autopilot_close setting).
// The autopilot setting allows it; the mode itself starts off.

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { execFile } = require("node:child_process");
const { STATE_DIR } = require("./config");
const { gitOf, dirsOf } = require("./landing");
const { spentToday } = require("./sessions");
const { isOn } = require("../settings");

const FILE = () => path.join(STATE_DIR, "autopilot.json");
const NOTES = () => process.env.NOTES_DIR || path.join(os.homedir(), "Notes");
const WTN_OPEN = () => process.env.TMUX_SIDEBAR_WTN_OPEN || path.join(__dirname, "../../tmux/wtn-open.zsh");
const DEFAULTS = { on: false, note: "", repo: "", slots: 3, cap: 5, paused: false, readyOnly: false, skipped: [], spendLimit: 0, said: "" };

// A tab not in its worktree yet counts as busy this long, while wtn makes
// it; past it, wtn failed, and the tab waits on you.
const OPENING_S = 10 * 60;
// A tab counts as busy this long after it opened whatever it shows, since
// Claude takes a moment to start in its worktree.
const STARTING_S = 2 * 60;
// Openings are this far apart, so wtn's worktrees and the new Claude
// sessions don't race.
const GAP_MS = 5_000;
// An item opened this recently is never taken again, before wtn has made
// its branch, which is what claims it.
const RECENT_MS = 60_000;
// A note changed this recently isn't taken from: an item being typed is
// saved as it is typed, and its first words would be opened as an item.
const SETTLE_MS = 60_000;
// The next item is read again at most this often, and when the note changes.
const QUEUE_MS = 30_000;
// A landed tab you never look at is closed this long after it landed.
const CLOSE_AFTER_S = 30 * 60;
// A landed tab in front this long counts as seen, so passing over it, as
// moving the sidebar's cursor does, doesn't.
const SEEN_MS = 1_000;

// Notes writes the file too, when it renames the note (store.relink), and
// Lua's JSON makes an empty list an object.
const read = () => {
  try {
    const s = { ...DEFAULTS, ...JSON.parse(fs.readFileSync(FILE(), "utf8")) };
    return { ...s, skipped: Array.isArray(s.skipped) ? s.skipped : [] };
  } catch {
    return { ...DEFAULTS };
  }
};
const write = (patch) => {
  const next = { ...read(), ...patch };
  fs.mkdirSync(STATE_DIR, { recursive: true });
  fs.writeFileSync(`${FILE()}.tmp`, JSON.stringify(next, null, 2));
  fs.renameSync(`${FILE()}.tmp`, FILE());
  return next;
};

// The main clone of a directory's repository, whichever worktree it is in.
const mainClone = (dir) => {
  const git = gitOf(dir);
  if (!git) return undefined;
  return path.basename(git.common) === ".git" ? path.dirname(git.common) : git.root;
};

const noteTitle = (note) => path.basename(note ?? "", ".md");

// Where one of the mode's tabs is, `now` in seconds:
// - "busy": working in its worktree, or still opening;
// - "waiting": waiting on you (a question, a turn ended, or wtn failed), which
//   frees its slot but still counts toward the cap;
// - "left": back in the main clone once queue-and-clean has landed it, which
//   leaves the mode.
// `inWorktree` says whether it is in a worktree of the repository now.
// `parked` are the windows holding a pane a tool stands in for
// (pane-in-place.sh, M-d or lazygit): while one stands in for Claude's pane,
// Claude's state is that window's, background work included, found by the
// pane the tool's pane names in @parked_pane.
const stateOf = (tab, parked = []) => {
  const pane = tab.panes.map((p) => p["@parked_pane"]).find(Boolean);
  return (pane && parked.find((t) => t.panes.some((p) => p.pane_id === pane))?.state) || tab.state;
};
const placeOf = (tab, repo, now, parked) => {
  const git = dirsOf(tab).map(gitOf).find(Boolean);
  const inWorktree = Boolean(git && git.root !== repo && mainClone(git.root) === repo);
  const age = now - tab.autopilot.since;
  // A tab waiting on you frees its slot at once; one idle or done counts as
  // busy for STARTING_S, while its Claude may not have started yet.
  const state = stateOf(tab, parked);
  if (inWorktree) return { inWorktree, kind: ["working", "background"].includes(state) || (state !== "waiting" && age < STARTING_S) ? "busy" : "waiting" };
  if (tab.autopilot.inWorktree) return { inWorktree, kind: "left" };
  return { inWorktree, kind: age < OPENING_S ? "busy" : "waiting" };
};

// What a tab of the mode back in the main clone gets, `now` in seconds:
// marked landed once its pane is in a main clone, marked seen once in front
// for SEEN_MS (`fronts`, when each was first found in front), and closed
// once seen and left, or CLOSE_AFTER_S after landing, unless a prompt typed
// since (a state begun after it landed) made it yours, or a tool stands in
// for one of its panes.
const unmark = (tab) => ["@autopilot", "@autopilot_in", "@autopilot_item", "@autopilot_left", "@autopilot_seen"].map((option) => ["set-option", "-w", "-u", "-t", tab.id, option]);
const afterLanding = (tab, now, parked, fronts) => {
  const { landed, seen } = tab.autopilot;
  if (!landed) {
    // Claude's own pane, idle, in a main clone, whichever: the mode may have
    // moved to another repository since it opened this tab. Only a directory
    // still there counts, and not another split's: a removed worktree under
    // the main clone (.claude/worktrees) would be found in the main clone,
    // and a shell beside it may be there already, while queue-and-clean is
    // still asking something before it moves Claude's pane.
    const claudePane = tab.panes.find((p) => p.pane_id === tab.claudePane);
    const dirs = claudePane ? [claudePane.pane_current_path] : dirsOf(tab);
    const root = dirs.filter((dir) => dir && fs.existsSync(dir)).map(gitOf).find(Boolean)?.root;
    const home = Boolean(root) && mainClone(root) === root && stateOf(tab, parked) === "idle";
    return home ? [["set-option", "-w", "-t", tab.id, "@autopilot_left", String(Math.floor(now))]] : [];
  }
  const standIn = tab.panes.map((p) => p["@parked_pane"]).find(Boolean);
  const panes = [...tab.panes, ...(parked.find((t) => t.panes.some((p) => p.pane_id === standIn))?.panes ?? [])];
  const promptedSince = stateOf(tab, parked) !== "idle" && panes.some((p) => Number(p["@agent_since"]) > landed);
  if (promptedSince) return unmark(tab);
  if (tab.current && tab.attached) {
    if (!fronts.has(tab.id)) fronts.set(tab.id, Date.now());
    return !seen && Date.now() - fronts.get(tab.id) >= SEEN_MS ? [["set-option", "-w", "-t", tab.id, "@autopilot_seen", "1"]] : [];
  }
  fronts.delete(tab.id);
  if (standIn || !(seen || now - landed >= CLOSE_AFTER_S)) return [];
  // With Notes moved beside it (M-e), only its own panes close: Notes, left
  // alone, goes back to its tab (notes-open.sh orphan, from after-kill-pane),
  // and the daemon closes the tab left holding only its sidebar.
  const notesBeside = tab.panes.some((p) => p["@notes_pane"] === "1");
  return notesBeside ? tab.panes.filter((p) => p["@notes_pane"] !== "1").map((p) => ["kill-pane", "-t", p.pane_id]) : [["kill-window", "-t", tab.id]];
};

// The mode, with what it keeps between checks. `batch` runs tmux commands,
// `log` writes the daemon's log and `changed` redraws; the tests give a
// shorter `gapMs`.
const autopilot = ({ batch, log = () => {}, changed = () => {}, gapMs = GAP_MS }) => {
  let opening = false;
  let lastOpen = 0;
  // When each landed tab was first found in front, for afterLanding.
  const fronts = new Map();
  const recent = new Map();
  // When the note was last found empty, and its modified time then: an
  // empty note is read again once it changes, or a minute later, since an
  // item a worktree had comes back once that worktree is removed.
  let emptyAt;
  let emptySince = 0;
  // The next item, as last read, and when, and from which note and time.
  let next = { text: "", at: 0, key: "" };
  let reading = false;
  // Said once a day.
  let spendSaid = "";
  // The note said to be missing, said once.
  let missingSaid = "";

  const noteMtime = (note) => {
    try {
      return fs.statSync(path.join(NOTES(), note)).mtimeMs;
    } catch {
      return 0;
    }
  };
  // The items of the mode's tabs still open, which are never taken again
  // while their tab is there, whether or not wtn made their branch.
  let held = [];
  const skip = (s) => [...s.skipped, ...held, ...[...recent].filter(([, at]) => Date.now() - at < RECENT_MS).map(([text]) => text)].join("\n");
  const run = (s, args, done, env = {}) =>
    execFile("zsh", [WTN_OPEN(), ...args, ...(s.readyOnly ? ["--ready-only"] : [])], { cwd: s.repo, timeout: 60_000, env: { ...process.env, WTN_SKIP: skip(s), ...env } }, done);

  // The item it will take next, read in the background, for mission control.
  const readNext = (s) => {
    const key = `${s.note}\t${noteMtime(s.note)}\t${skip(s)}\t${s.readyOnly}`;
    if (reading || (key === next.key && Date.now() - next.at < QUEUE_MS)) return;
    reading = true;
    run(s, ["queue", s.note], (err, out) => {
      reading = false;
      const text = err ? "" : (out.split("\n")[0] ?? "").split("\t").slice(1).join("\t");
      next = { text, at: Date.now(), key };
      changed();
    });
  };

  // Each check, on every refresh and once a minute: marks the mode's tabs as
  // they reach their worktree or leave the mode, then opens the next item
  // while fewer than `slots` work and fewer than `cap` are open. `away` is
  // whether you're away from the Mac; `session` the tmux session to open in.
  const check = (tabs, { parked, away, session } = {}) => {
    const s = read();
    if (!s.repo) return;
    const now = Date.now() / 1000;
    const mine = tabs.filter((t) => t.autopilot).map((tab) => ({ tab, ...placeOf(tab, s.repo, now, parked) }));
    const closing = isOn("autopilot_close");
    const marks = mine.flatMap(({ tab, inWorktree, kind }) =>
      kind === "left"
        ? closing
          ? afterLanding(tab, now, parked ?? [], fronts)
          : unmark(tab)
        : inWorktree && !tab.autopilot.inWorktree
          ? [["set-option", "-w", "-t", tab.id, "@autopilot_in", "1"]]
          : [],
    );
    [...fronts.keys()].filter((id) => !tabs.some((t) => t.id === id)).forEach((id) => fronts.delete(id));
    if (marks.length) batch(marks);
    held = mine.filter((m) => m.kind !== "left" && m.tab.autopilot.item).map((m) => m.tab.autopilot.item);
    if (!isOn("autopilot") || !s.on || !s.note) return;
    readNext(s);
    const open = mine.filter((m) => m.kind !== "left");
    const working = open.filter((m) => m.kind === "busy");
    if (s.paused || away || opening || Date.now() - lastOpen < gapMs) return;
    if (working.length >= s.slots || open.length >= s.cap) return;
    const today = new Date().toDateString();
    if (s.spendLimit && spentToday() >= s.spendLimit) {
      if (spendSaid !== today) {
        spendSaid = today;
        write({ said: `Stopped opening tabs: today's Claude spend passed $${s.spendLimit}` });
        changed();
      }
      return;
    }
    const mtime = noteMtime(s.note);
    if (!mtime) {
      if (missingSaid !== s.note) {
        missingSaid = s.note;
        write({ said: `Can't find the note ${noteTitle(s.note)}: set the mode up again from M-p` });
        changed();
      }
      return;
    }
    if (Date.now() - mtime < SETTLE_MS) return;
    if (emptyAt === mtime && Date.now() - emptySince < RECENT_MS) return;
    opening = true;
    lastOpen = Date.now();
    run(s, ["next", s.note, "1"], (err, out, stderr) => {
      opening = false;
      if (err) {
        log(`autopilot: ${String(stderr || err.message).trim()}`);
        return;
      }
      const rows = out.split("\n").filter(Boolean).map((row) => row.split("\t"));
      if (!rows.length) {
        // Said once, when it first finds the note empty.
        if (emptyAt === undefined) write({ said: `Nothing left to take in ${noteTitle(s.note)}` });
        emptyAt = mtime;
        emptySince = Date.now();
      } else {
        emptyAt = undefined;
        rows.forEach(([line, window, ...text]) => {
          recent.set(text.join("\t"), Date.now());
          log(`autopilot: opened line ${line} of ${s.note} in ${window}`);
        });
        if (/^Nothing left/.test(read().said)) write({ said: "" });
      }
      next.at = 0;
      changed();
    }, session ? { WTN_SESSION: session } : {});
  };

  // What mission control shows while the mode is on: the note, the slots
  // busy and tabs open, the next item, and whether it is paused. Kept for a
  // second for the same tabs, since mission control draws with every turn of
  // its spinner.
  let shown = { at: 0 };
  const summary = (tabs, { parked, away } = {}) => {
    if (shown.tabs === tabs && shown.away === away && shown.next === next.text && Date.now() - shown.at < 1000) return shown.value;
    shown = { at: Date.now(), tabs, away, next: next.text, value: summarize(tabs, away, parked) };
    return shown.value;
  };
  const summarize = (tabs, away, parked) => {
    const s = read();
    if (!isOn("autopilot") || !s.on) return undefined;
    const now = Date.now() / 1000;
    const mine = tabs.filter((t) => t.autopilot).map((tab) => placeOf(tab, s.repo, now, parked).kind).filter((kind) => kind !== "left");
    return {
      note: noteTitle(s.note),
      busy: mine.filter((kind) => kind === "busy").length,
      open: mine.length,
      slots: s.slots,
      cap: s.cap,
      next: next.text,
      paused: s.paused,
      away: Boolean(away),
      readyOnly: s.readyOnly,
      skipped: s.skipped.length,
      spendLimit: s.spendLimit,
    };
  };

  // What Enter on mission control's line offers. `setup` asks for the note,
  // slots and cap again.
  const choices = (setup) => {
    const s = read();
    const done = (patch, note) => () => {
      write(patch);
      next.at = 0;
      emptyAt = undefined;
      shown.at = 0;
      changed();
      return note;
    };
    return [
      s.paused ? { key: "r", label: "Resume", act: done({ paused: false }, "Resumed") } : { key: "p", label: "Pause: open no more tabs for now", act: done({ paused: true }, "Paused") },
      ...(next.text ? [{ key: "s", label: `Skip the next item: ${next.text}`, act: done({ skipped: [...s.skipped, next.text] }, `Skipped: ${next.text}`) }] : []),
      ...(s.skipped.length ? [{ key: "c", label: `Take the ${s.skipped.length} skipped item${s.skipped.length === 1 ? "" : "s"} again`, act: done({ skipped: [] }, "The skipped items are back in line") }] : []),
      s.readyOnly
        ? { key: "a", label: "Take items without a plan too", act: done({ readyOnly: false }, "Taking items without a plan too, after the planned ones") }
        : { key: "a", label: "Take only items with a settled plan", act: done({ readyOnly: true }, "Taking only items with a settled plan") },
      { key: "n", label: "Change its note, slots, cap or spend limit…", act: setup },
      { key: "x", label: "Stop working the note", act: done({ on: false, paused: false, said: "" }, "Stopped: no more tabs open for the note; those open carry on") },
    ];
  };

  return { check, summary, choices };
};

// The notes, the one the mode last worked first, then the most recently
// changed: paths within the notes directory, leaving out deleted ones, those
// holding an unticked item (`open`) apart from the rest.
const noteChoices = () => {
  const dir = NOTES();
  const walk = (folder) => {
    try {
      return fs.readdirSync(folder, { withFileTypes: true }).flatMap((entry) => {
        if (entry.name.startsWith(".")) return [];
        const full = path.join(folder, entry.name);
        if (entry.isDirectory()) return walk(full);
        return entry.name.endsWith(".md") ? [{ note: path.relative(dir, full), mtime: fs.statSync(full).mtimeMs, open: /^\s*- \[ \] /m.test(fs.readFileSync(full, "utf8")) }] : [];
      });
    } catch {
      return [];
    }
  };
  const last = read().note;
  const notes = walk(dir).sort((a, b) => (b.note === last) - (a.note === last) || b.mtime - a.mtime);
  return { open: notes.filter((n) => n.open).map((n) => n.note), rest: notes.filter((n) => !n.open).map((n) => n.note) };
};

module.exports = { autopilot, readAutopilot: read, writeAutopilot: write, mainClone, noteChoices, noteTitle, placeOf, OPENING_S, STARTING_S, CLOSE_AFTER_S };
