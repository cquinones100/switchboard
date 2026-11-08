// Part of bin/tmux-sidebar; see the comment at the top of that file.
//
// Working a note's checklist continuously: with the mode on (M-p's "Work the
// note continuously"), the daemon keeps `slots` tabs working through a note's
// items, each opened with wtn (tmux/wtn-open.zsh next), and opens the next
// whenever one lands. A tab waiting on you frees its slot but still counts
// toward `cap`, the most of the mode's tabs open at once. Its state, what
// you chose, is kept in STATE_DIR/autopilot.json:
// { on, note, noteId, repo, slots, cap, paused, readyOnly, skipped, spendLimit,
// landWithoutAsking, said, closed: [{ id, item, hash, at }] }, each closed
// `id` the window's and its @autopilot time, since tmux gives a window's ID
// again after a restart.
// `note` is the note's path within the notes directory and `noteId` its
// stable ID (lib/note-id.js): each check finds the note by its ID when it has
// been renamed or moved, outside Notes too, and writes its path back.
// `skipped` holds { note, text }, an item's note's ID and its first line, so
// a skipped item hides only in its own note.
// Each tab it opens is marked @autopilot with the time it opened, and
// @autopilot_in once seen in its worktree, so it counts only its own tabs;
// @autopilot_item and @autopilot_note name the item it is for and its note's
// ID, which hold the item while the tab is open.
// Once queue-and-clean has landed one and moved it back to the main clone,
// it is marked @autopilot_left with the time, @autopilot_seen once you have
// had it in front (here, or agent-state.sh seen), and closed once you leave
// it, or the autopilot_close setting's delay after landing (CLOSE_AFTER_S)
// if you never look, which leaves a notification (`closed` in the state); a
// prompt typed in it since makes it yours, out of the mode.
// With "Land without asking" on (landWithoutAsking), each of its tabs still
// working is marked @autopilot_land, which queue-and-clean reads to land
// on the default branch without asking to push, and which the switch sets
// and clears on the open tabs as it changes; queue-and-clean marks one that
// landed so @autopilot_autoland, with the commit (land-lock.js autolanded).
// The autopilot setting allows it; the mode itself starts off.

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { execFile } = require("node:child_process");
const { STATE_DIR } = require("./config");
const { gitOf, dirsOf } = require("./landing");
const { spentToday } = require("./sessions");
const { headOf } = require("./story-ahead");
const { isOn, currentSettings, nextValue, reloadSettings, valueWords, writeSetting } = require("../settings");
const { ensureId, findById, giveId, idOf, migrateClaims } = require("../note-id");

const FILE = () => path.join(STATE_DIR, "autopilot.json");
const NOTES = () => process.env.NOTES_DIR || path.join(os.homedir(), "Notes");
const WTN_OPEN = () => process.env.TMUX_SIDEBAR_WTN_OPEN || path.join(__dirname, "../../tmux/wtn-open.zsh");
const DEFAULTS = { on: false, note: "", noteId: "", repo: "", slots: 3, cap: 5, paused: false, readyOnly: false, skipped: [], spendLimit: 0, landWithoutAsking: false, said: "", closed: [] };

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
// A landed tab's item is held until the note shows it ticked, or this long,
// so a tick that never comes doesn't hold it for good.
const LANDED_MS = 10 * 60_000;
// A note changed this recently isn't taken from: an item being typed is
// saved as it is typed, and its first words would be opened as an item.
const SETTLE_MS = 60_000;
// The next item is read again at most this often, and when the note changes.
const QUEUE_MS = 30_000;
// A landed tab you never look at is closed this long after it landed, by the
// autopilot_close setting; "off" never closes one.
const CLOSE_AFTER_S = { seen: 30 * 60, "5m": 5 * 60, now: 0 };
// The tabs closed without being seen, kept for the notifications.
const CLOSED_KEPT = 20;
// A landed tab in front this long counts as seen, so passing over it, as
// moving the sidebar's cursor does, doesn't.
const SEEN_MS = 1_000;

// Notes writes the file too, when it renames the note (store.relink), and
// Lua's JSON makes an empty list an object. A skipped item from before notes
// had IDs, its first line alone, was the mode's note's.
const read = () => {
  try {
    const s = { ...DEFAULTS, ...JSON.parse(fs.readFileSync(FILE(), "utf8")) };
    const skipped = (Array.isArray(s.skipped) ? s.skipped : []).map((x) => (typeof x === "string" ? { note: s.noteId, text: x } : x));
    return { ...s, skipped, closed: Array.isArray(s.closed) ? s.closed : [] };
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

// The skipped items of the mode's note.
const skippedHere = (s) => s.skipped.filter((x) => x.note === s.noteId);

// How often a note lost by its ID is looked for again.
const LOST_MS = 30_000;

// Where one of the mode's tabs is, `now` in seconds:
// - "busy": working in its worktree, or still opening;
// - "waiting": waiting on you (a question, a turn ended, or wtn failed), which
//   frees its slot but still counts toward the cap;
// - "landing": out of its worktree but not back in a main clone, as while
//   queue-and-clean removes the worktree its pane is still in, which holds its
//   slot and its item as "waiting" does;
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
// A directory that is gone counts for nothing: a removed worktree under the
// main clone's .claude/worktrees would otherwise read as the main clone.
const placeOf = (tab, repo, now, parked) => {
  const git = dirsOf(tab).filter((dir) => fs.existsSync(dir)).map(gitOf).find(Boolean);
  const inWorktree = Boolean(git && git.root !== repo && mainClone(git.root) === repo);
  const age = now - tab.autopilot.since;
  // A tab waiting on you frees its slot at once; one idle or done counts as
  // busy for STARTING_S, while its Claude may not have started yet.
  const state = stateOf(tab, parked);
  if (inWorktree) return { inWorktree, kind: ["working", "background"].includes(state) || (state !== "waiting" && age < STARTING_S) ? "busy" : "waiting" };
  if (tab.autopilot.inWorktree) return { inWorktree, kind: git && mainClone(git.root) === git.root ? "left" : "landing" };
  return { inWorktree, kind: age < OPENING_S ? "busy" : "waiting" };
};

// What a tab of the mode back in the main clone gets, `now` in seconds:
// marked landed once its pane is in a main clone, marked seen once in front
// for SEEN_MS (`fronts`, when each was first found in front), and closed
// once seen and left, or `closeAfter` seconds after landing, unless a prompt typed
// since (a state begun after it landed) made it yours, or a tool stands in
// for one of its panes.
const unmark = (tab) => ["@autopilot", "@autopilot_in", "@autopilot_item", "@autopilot_note", "@autopilot_left", "@autopilot_seen", "@autopilot_land", "@autopilot_autoland"].map((option) => ["set-option", "-w", "-u", "-t", tab.id, option]);
const afterLanding = (tab, now, parked, fronts, closeAfter) => {
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
  if (standIn || !(seen || now - landed >= closeAfter)) return [];
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
  // When the note, lost by its ID, was last looked for, and whether it was
  // found then.
  let lostAt = 0;
  let lost = false;
  // The repositories whose wtn claims have been brought to IDs.
  const migrated = new Set();

  // The mode's note where it is now: one from before notes had IDs given its
  // ID, its skipped items with it; one no longer at its path found by its ID
  // and its path written back, so a rename anywhere is followed. The note at
  // its path with no ID, rewritten outside Notes without its last line, is
  // given it back; one holding another note's ID, or none there, leaves the
  // note `lost`, and the mode takes nothing from another note in its place.
  const follow = (s) => {
    if (!s.note) return s;
    const file = path.join(NOTES(), s.note);
    if (!s.noteId) {
      if (!fs.existsSync(file)) return s;
      let noteId;
      try {
        noteId = ensureId(file, NOTES());
      } catch {
        return s;
      }
      return write({ noteId, skipped: s.skipped.map((x) => (x.note ? x : { ...x, note: noteId })) });
    }
    const here = idOf(file);
    if (here === s.noteId) {
      if (!lost) return s;
      lost = false;
      // Said again if it goes missing again.
      missingSaid = "";
      return /^Can't find the note/.test(s.said) ? write({ said: "" }) : s;
    }
    // Looked for again only every LOST_MS, but another note's ID at its path
    // is never worked meanwhile.
    if (Date.now() - lostAt < LOST_MS) return { ...s, lost: lost || Boolean(here) };
    const found = findById(s.noteId, NOTES());
    if (!found) {
      lostAt = Date.now();
      lost = true;
      if (!here && fs.existsSync(file)) {
        try {
          giveId(file, s.noteId);
          lost = false;
          lostAt = 0;
          missingSaid = "";
          if (/^Can't find the note/.test(s.said)) return write({ said: "" });
        } catch {}
      }
      return { ...s, lost };
    }
    lostAt = 0;
    lost = false;
    missingSaid = "";
    log(`autopilot: ${s.note} is now ${path.relative(NOTES(), found)}`);
    return write({ note: path.relative(NOTES(), found), said: /^Can't find the note/.test(s.said) ? "" : s.said });
  };

  // The items of the mode's tabs that left, and when, by their note's ID and
  // first line: held until the note no longer lists them unticked, or
  // LANDED_MS.
  const landed = new Map();
  const keyOf = (note, text) => `${note}\t${text}`;
  let noteRead = { key: "", items: new Set() };
  const unticked = (note) => {
    const key = `${note}\t${noteMtime(note)}`;
    if (key !== noteRead.key) {
      let text = "";
      try {
        text = fs.readFileSync(path.join(NOTES(), note), "utf8");
      } catch {}
      noteRead = { key, items: new Set(text.split("\n").filter((line) => line.startsWith("- [ ] ")).map((line) => line.slice(6).trim())) };
    }
    return noteRead.items;
  };

  const noteMtime = (note) => {
    try {
      return fs.statSync(path.join(NOTES(), note)).mtimeMs;
    } catch {
      return 0;
    }
  };
  // The items of the mode's tabs still open, which are never taken again
  // while their tab is there, whether or not wtn made their branch, and those
  // opened a moment ago (`recent`), each by its note's ID and first line;
  // only the mode's note's are skipped.
  let held = [];
  const skip = (s) => {
    const here = (key) => key.startsWith(keyOf(s.noteId, ""));
    const opened = [...recent].filter(([key, at]) => here(key) && Date.now() - at < RECENT_MS).map(([key]) => key);
    return [...skippedHere(s).map((x) => x.text), ...[...held, ...opened].filter(here).map((key) => key.slice(keyOf(s.noteId, "").length))].join("\n");
  };
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
    const s = follow(read());
    if (!s.repo) return;
    // wtn's claims from before notes had IDs, brought to IDs once a run.
    if (!migrated.has(s.repo)) {
      migrated.add(s.repo);
      try {
        for (const claim of migrateClaims(s.repo, NOTES())) log(`autopilot: couldn't find the note of the claim "${claim}", left as it was`);
      } catch (err) {
        log(`autopilot: bringing wtn's claims to note IDs: ${err.message}`);
      }
    }
    const now = Date.now() / 1000;
    const mine = tabs.filter((t) => t.autopilot).map((tab) => ({ tab, ...placeOf(tab, s.repo, now, parked) }));
    const closeAfter = CLOSE_AFTER_S[closeSetting()];
    const closedNow = [];
    // Its tabs not landed yet land without asking while the switch is on.
    const land = isOn("autopilot") && s.landWithoutAsking;
    const mark = (id, has) => (land === has ? [] : land ? [["set-option", "-w", "-t", id, "@autopilot_land", "1"]] : [["set-option", "-w", "-u", "-t", id, "@autopilot_land"]]);
    // The skills read it from the window of Claude's pane, which a tool in
    // its place (pane-in-place.sh, M-d or lazygit) has parked in a window of
    // its own, so that window is marked too while it holds it.
    const landMark = (tab) => {
      const parkedPane = tab.panes.map((p) => p["@parked_pane"]).find(Boolean);
      const holder = parkedPane && (parked ?? []).find((t) => t.panes.some((p) => p.pane_id === parkedPane));
      return [...mark(tab.id, Boolean(tab.autopilot.land)), ...(holder ? mark(holder.id, holder.panes.some((p) => p["@autopilot_land"] === "1")) : [])];
    };
    const marks = mine.flatMap(({ tab, inWorktree, kind }) => {
      if (kind !== "left") return [...(inWorktree && !tab.autopilot.inWorktree ? [["set-option", "-w", "-t", tab.id, "@autopilot_in", "1"]] : []), ...landMark(tab)];
      if (closeAfter === undefined) return unmark(tab);
      const commands = afterLanding(tab, now, parked ?? [], fronts, closeAfter);
      if (!tab.autopilot.seen && commands.some((c) => c[0].startsWith("kill-"))) closedNow.push(tab);
      return commands;
    });
    if (closedNow.length) {
      const was = read().closed;
      const idOf = (tab) => `${tab.id}@${tab.autopilot.since}`;
      const fresh = closedNow.filter((tab) => !was.some((c) => c.id === idOf(tab)));
      // What landed: the main clone's commit as Claude's pane finds it there.
      const hashOf = (tab) => headOf(tab.panes.find((p) => p.pane_id === tab.claudePane)?.pane_current_path ?? dirsOf(tab)[0] ?? "")?.sha.slice(0, 7) ?? "";
      if (fresh.length) write({ closed: [...was, ...fresh.map((tab) => ({ id: idOf(tab), item: tab.autopilot.item ?? tab.name ?? "", hash: hashOf(tab), at: Date.now() }))].slice(-CLOSED_KEPT) });
      changed();
    }
    [...fronts.keys()].filter((id) => !tabs.some((t) => t.id === id)).forEach((id) => fronts.delete(id));
    if (marks.length) batch(marks);
    // A landed tab stays marked until it closes, so its item's time is when
    // it was first found left, forgotten once ticked, or once past
    // LANDED_MS with its tab gone.
    // A tab from before tabs kept their note's ID gets the ID of the note it
    // was opened for (@tab_note, a path in the notes directory, given an ID
    // when it has none), kept on it now, so it stays that note's once the
    // note is renamed or the mode works another; one with no @tab_note is
    // the mode's note's, and one whose note is gone is no note's held item.
    const idFor = (tabNote) => {
      if (!tabNote || tabNote === s.note) return s.noteId;
      try {
        return ensureId(path.join(NOTES(), tabNote), NOTES());
      } catch {
        return "";
      }
    };
    const unnoted = mine
      .filter((m) => !m.tab.autopilot.note && m.tab.autopilot.item)
      .map((m) => ({ m, id: idFor(m.tab.autopilot.tabNote) }))
      .filter(({ id }) => id);
    if (unnoted.length) {
      unnoted.forEach(({ m, id }) => (m.tab.autopilot.note = id));
      batch(unnoted.map(({ m, id }) => ["set-option", "-w", "-t", m.tab.id, "@autopilot_note", id]));
    }
    const itemOf = (tab) => keyOf(tab.autopilot.note || (tab.autopilot.tabNote && tab.autopilot.tabNote !== s.note ? `gone:${tab.autopilot.tabNote}` : s.noteId), tab.autopilot.item);
    const leftItems = new Set(mine.filter((m) => m.kind === "left" && m.tab.autopilot.item).map((m) => itemOf(m.tab)));
    for (const key of leftItems) if (!landed.has(key)) landed.set(key, Date.now());
    if (landed.size) {
      const open = unticked(s.note);
      const ticked = (key) => key.startsWith(keyOf(s.noteId, "")) && !open.has(key.slice(keyOf(s.noteId, "").length));
      for (const [key, at] of landed) if (ticked(key) || (Date.now() - at >= LANDED_MS && !leftItems.has(key))) landed.delete(key);
    }
    held = [...mine.filter((m) => m.kind !== "left" && m.tab.autopilot.item).map((m) => itemOf(m.tab)), ...[...landed].filter(([, at]) => Date.now() - at < LANDED_MS).map(([key]) => key)];
    if (!isOn("autopilot") || !s.on || !s.note) return;
    if (s.lost) {
      if (missingSaid !== s.noteId) {
        missingSaid = s.noteId;
        write({ said: `Can't find the note ${noteTitle(s.note)}: set the mode up again from M-p` });
        changed();
      }
      return;
    }
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
          recent.set(keyOf(s.noteId, text.join("\t")), Date.now());
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
      skipped: skippedHere(s).length,
      spendLimit: s.spendLimit,
      landsAlone: s.landWithoutAsking,
    };
  };

  // What Enter on mission control's line offers. `setup` asks for the note,
  // slots and cap again; `confirm(text, list, yes)` asks a yes/no question.
  const choices = (setup, confirm) => {
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
      ...(next.text ? [{ key: "s", label: `Skip the next item: ${next.text}`, act: done({ skipped: [...s.skipped, { note: s.noteId, text: next.text }] }, `Skipped: ${next.text}`) }] : []),
      ...(skippedHere(s).length ? [{ key: "c", label: `Take the ${skippedHere(s).length} skipped item${skippedHere(s).length === 1 ? "" : "s"} again`, act: done({ skipped: s.skipped.filter((x) => x.note !== s.noteId) }, "The skipped items are back in line") }] : []),
      s.readyOnly
        ? { key: "a", label: "Take items without a plan too", act: done({ readyOnly: false }, "Taking items without a plan too, after the planned ones") }
        : { key: "a", label: "Take only items with a settled plan", act: done({ readyOnly: true }, "Taking only items with a settled plan") },
      { key: "l", label: `Close landed tabs: ${valueWords(closeOption())}`, act: cycleClose },
      s.landWithoutAsking
        ? { key: "w", label: "Ask before landing", act: done({ landWithoutAsking: false }, "Its tabs ask before they land again; those open now too") }
        : {
            key: "w",
            label: "Land without asking",
            act: () =>
              confirm(
                "Land its tabs on the default branch without asking to push? [Y/n]",
                [
                  "Skipped: the question before each push, and medium review findings, fixed without asking",
                  "Kept: the tests, two clean review rounds, the landing lock, leak-check; a repository whose landing runs no tests or no review still asks",
                  "Asked after all: anything leak-check finds, an unclear conflict, a failure the branch didn't cause",
                  "Told after each landing, in the notifications and on Slack",
                  "Only for landings straight on the default branch; tabs open now too",
                ],
                done({ landWithoutAsking: true }, "Its tabs land without asking; you're told after each"),
              ),
          },
      { key: "n", label: "Change its note, slots, cap or spend limit…", act: setup },
      { key: "x", label: "Stop working the note", act: done({ on: false, paused: false, said: "" }, "Stopped: no more tabs open for the note; those open carry on") },
    ];
  };

  return { check, summary, choices };
};

// The autopilot_close setting, as last read, and its choice.
const closeOption = () => currentSettings().find((option) => option.key === "autopilot_close");
const closeSetting = () => closeOption()?.value;
// The mode's menu changes it to its next choice, as Settings does; the
// daemon's watch on the settings file applies it.
const cycleClose = () => {
  const value = nextValue(closeOption());
  try {
    writeSetting("autopilot_close", value);
  } catch (err) {
    return `Couldn't save it: ${err.message}`;
  }
  reloadSettings();
  return `Landed tabs close: ${valueWords(closeOption())}`;
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

// The landings made without asking, for the notifications: what
// queue-and-clean's land-lock.js autolanded appends to autolanded.json,
// [{ hash, item, window, at }], until each is dismissed.
const LANDED_FILE = () => path.join(STATE_DIR, "autolanded.json");
const readLanded = () => {
  try {
    const list = JSON.parse(fs.readFileSync(LANDED_FILE(), "utf8"));
    return Array.isArray(list) ? list.filter((l) => l && typeof l.hash === "string") : [];
  } catch {
    return [];
  }
};
const dismissLanded = (hash) => {
  const rest = readLanded().filter((l) => l.hash !== hash);
  try {
    fs.writeFileSync(`${LANDED_FILE()}.tmp`, JSON.stringify(rest));
    fs.renameSync(`${LANDED_FILE()}.tmp`, LANDED_FILE());
  } catch {}
};

module.exports = { autopilot, notesDir: NOTES, readAutopilot: read, writeAutopilot: write, readLanded, dismissLanded, mainClone, noteChoices, noteTitle, placeOf, OPENING_S, STARTING_S, CLOSE_AFTER_S };
