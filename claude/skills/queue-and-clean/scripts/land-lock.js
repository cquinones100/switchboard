#!/usr/bin/env node
// The landing lock of queue-and-clean's "Land on the default branch": one
// worktree of a repository lands on its default branch at a time, so two
// never rebase onto the same one and push over each other. Each tests and
// reviews before taking it. It lives in the main clone's git directory,
// shared by every worktree of it, and so does the queue of the worktrees waiting for it, which lands them in its order.
//
//   land-lock.js take      takes it for this worktree: exit 0 taken (or held
//                          by this worktree already, so a landing checks it
//                          still holds it after a wait), 3 held by another or
//                          others ahead in the queue, which it prints, this
//                          worktree joining the queue
//   land-lock.js wait      takes it, waiting in the queue for its turn; exit 4
//                          when taken out of the queue meanwhile
//   land-lock.js release   lets it go, when this worktree holds it, and takes
//                          this worktree out of the queue
//   land-lock.js leave     takes this worktree out of the queue
//   land-lock.js holder    prints which worktree holds it, nothing when none
//   land-lock.js queue     prints the holder ("landing") and each waiter by
//                          its place, one a line: <place>\t<path>\t<since>
//   land-lock.js move <path> up|down|first
//                          moves a waiter in the queue
//   land-lock.js phase [words]
//                          shows the landing's step in this tab, the pane's
//                          @agent_phase, its loop marked queue-and-clean's
//                          (@agent_loop), since a session following the skill
//                          by reading it never invokes it; no words clear it
//   land-lock.js autolanded <short hash> [item]
//                          tells the sidebar this tab landed without asking
//                          (the work-a-note mode's "Land without asking"):
//                          marks its window @autopilot_autoland with the hash
//                          and adds the landing to the notifications
//                          (autolanded.json in the sidebar's state directory)
//
// A landing holds it only to rebase, test and push, but one whose change no
// longer matches its review reviews again and asks again while holding it,
// which can wait on you for as long as you like, so the lock never expires
// with time: it is taken over only from a landing that
// is gone, its worktree removed or no Claude session running in it. A waiter
// gone the same way is dropped from the queue as it is read.

const { execFileSync, spawnSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { liveSessionsIn } = require("../../../../lib/worktree");

const git = (...args) => execFileSync("git", args, { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();

// The phase needs no repository, so it is set before the lookup below:
// the landing clears it last, after its cleanup has removed the worktree.
const phase = () => {
  const words = process.argv.slice(3).join(" ").replace(/\s+/g, " ").trim();
  const pane = process.env.TMUX_PANE;
  if (!pane || !process.env.TMUX) return;
  spawnSync("tmux", words ? ["set", "-p", "-t", pane, "@agent_phase", words, ";", "set", "-p", "-t", pane, "@agent_loop", "queue"] : ["set", "-pu", "-t", pane, "@agent_phase"], { stdio: "ignore" });
  const state = path.join(os.homedir(), ".local/state/tmux-sidebar");
  if (fs.existsSync(state)) fs.appendFileSync(path.join(state, "nudge"), "\n");
};
if (process.argv[2] === "phase") {
  phase();
  process.exit(0);
}

// Needs no repository either: it runs once the landing has pushed, whatever
// the cleanup has removed since.
const autolanded = () => {
  const [hash, ...item] = process.argv.slice(3);
  if (!hash) {
    console.error("usage: land-lock.js autolanded <short hash> [item]");
    return 2;
  }
  const pane = process.env.TMUX_PANE;
  let window = "";
  if (pane && process.env.TMUX) {
    // With a tool in its place (pane-in-place.sh, M-d or lazygit), Claude's
    // pane is parked in a window that is no tab: the tab is the window of the
    // tool's pane, which names Claude's in @parked_pane.
    const panes = spawnSync("tmux", ["list-panes", "-a", "-F", "#{pane_id}\t#{window_id}\t#{@parked_pane}"], { encoding: "utf8" }).stdout ?? "";
    const rows = panes.split("\n").map((row) => row.split("\t"));
    window = (rows.find((row) => row[2] === pane) ?? rows.find((row) => row[0] === pane))?.[1] ?? "";
    if (window) spawnSync("tmux", ["set", "-w", "-t", window, "@autopilot_autoland", hash], { stdio: "ignore" });
  }
  const dir = process.env.TMUX_SIDEBAR_STATE_DIR || path.join(os.homedir(), ".local/state/tmux-sidebar");
  const file = path.join(dir, "autolanded.json");
  let list = [];
  try {
    list = JSON.parse(fs.readFileSync(file, "utf8"));
  } catch {}
  if (!Array.isArray(list)) list = [];
  list = [...list.filter((l) => l?.hash !== hash), { hash, item: item.join(" ").replace(/\s+/g, " ").trim(), window, at: Date.now() }].slice(-20);
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(`${file}.tmp`, JSON.stringify(list));
  fs.renameSync(`${file}.tmp`, file);
  fs.appendFileSync(path.join(dir, "nudge"), "\n");
  return 0;
};
if (process.argv[2] === "autolanded") process.exit(autolanded());

let common;
let here;
try {
  common = git("rev-parse", "--path-format=absolute", "--git-common-dir");
  here = git("rev-parse", "--show-toplevel");
} catch {
  console.error("not in a git repository");
  process.exit(2);
}
const lock = path.join(common, "land.lock");
const holderFile = path.join(lock, "holder");
const queueFile = path.join(common, "land.queue");
const queueLock = path.join(common, "land.queue.lock");

// The sidebar shows the queue (lib/sidebar/landing.js): every change to it
// or the lock tells the sidebar daemon to look again.
const nudge = () => {
  const dir = process.env.TMUX_SIDEBAR_STATE_DIR || path.join(os.homedir(), ".local/state/tmux-sidebar");
  try {
    if (fs.existsSync(dir)) fs.appendFileSync(path.join(dir, "nudge"), "\n");
  } catch {}
};

const holder = () => {
  try {
    return JSON.parse(fs.readFileSync(holderFile, "utf8"));
  } catch {
    return undefined;
  }
};

const hold = () => fs.writeFileSync(holderFile, JSON.stringify({ path: here, since: Date.now() }));

// The worktree a directory is in: the nearest one up holding a .git. A
// session in a worktree nested inside another (the main clone's
// .claude/worktrees) is that one's, not the outer one's.
const worktreeOf = (dir) => {
  for (let at = dir; ; at = path.dirname(at)) {
    if (fs.existsSync(path.join(at, ".git"))) {
      try {
        return fs.realpathSync(at);
      } catch {
        return at;
      }
    }
    if (path.dirname(at) === at) return undefined;
  }
};

const gone = (current) => !current || !fs.existsSync(current.path) || !liveSessionsIn(current.path).some((session) => worktreeOf(session.cwd) === current.path);

const sleep = (ms) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);

// Every change to the queue holds a short mutex, so two worktrees joining at
// once can't lose one; one left behind by a killed process for over ten
// seconds is taken over. The queue is written whole and renamed into place.
const withQueue = (change) => {
  for (let tries = 0; ; tries++) {
    try {
      fs.mkdirSync(queueLock);
      break;
    } catch (err) {
      if (err.code !== "EEXIST") throw err;
      try {
        if (Date.now() - fs.statSync(queueLock).mtimeMs > 10_000) fs.rmSync(queueLock, { recursive: true, force: true });
      } catch {}
      if (tries > 500) throw new Error("the landing queue stayed locked");
      sleep(20);
    }
  }
  try {
    let read = [];
    try {
      read = JSON.parse(fs.readFileSync(queueFile, "utf8"));
    } catch {}
    const queue = (Array.isArray(read) ? read : []).filter((entry) => entry?.path && !gone(entry));
    const { queue: next = queue, result } = change(queue);
    if (JSON.stringify(next) !== JSON.stringify(read)) {
      const temporary = `${queueFile}.${process.pid}`;
      fs.writeFileSync(temporary, JSON.stringify(next));
      fs.renameSync(temporary, queueFile);
      nudge();
    }
    return result;
  } finally {
    fs.rmSync(queueLock, { recursive: true, force: true });
  }
};

const without = (queue, where) => queue.filter((entry) => entry.path !== where);
const joined = (queue) => (queue.some((entry) => entry.path === here) ? queue : [...queue, { path: here, since: Date.now() }]);

// Taken only when the lock is free (or its holder gone) and no live waiter is
// ahead of this worktree in the queue, so a take can't jump the queue.
const take = (quiet) =>
  withQueue((queue) => {
    const current = holder();
    if (current?.path === here && fs.existsSync(lock)) return { queue: without(queue, here), result: 0 };
    const mine = queue.findIndex((entry) => entry.path === here);
    const ahead = mine === -1 ? queue.length : mine;
    const free = !fs.existsSync(lock) || gone(current);
    if (free && ahead === 0) {
      try {
        fs.mkdirSync(lock);
      } catch (err) {
        if (err.code !== "EEXIST") throw err;
      }
      hold();
      nudge();
      return { queue: without(queue, here), result: 0 };
    }
    if (!quiet) {
      const held = free ? "" : `held by ${current.path} since ${new Date(current.since).toTimeString().slice(0, 5)}`;
      const waiting = ahead ? `${ahead} ahead in the landing queue` : "";
      console.log([held, waiting].filter(Boolean).join("; "));
    }
    return { queue: joined(queue), result: 3 };
  });

const ordinal = (n) => `${n}${n % 100 >= 11 && n % 100 <= 13 ? "th" : ["th", "st", "nd", "rd"][n % 10] ?? "th"}`;

const main = async () => {
  const command = process.argv[2];
  if (command === "take") process.exit(take());
  // A waiter taken out of the queue by someone else (mission control's
  // Landing view) stops, exit 4, rather than joining again at the back.
  if (command === "wait") {
    const poll = Number(process.env.LAND_LOCK_POLL ?? 5) * 1000;
    for (let waited = false; ; waited = true) {
      if (waited && !withQueue((queue) => ({ result: queue.some((entry) => entry.path === here) }))) {
        console.log("taken out of the landing queue");
        process.exit(4);
      }
      if (take(true) === 0) return;
      await new Promise((resolve) => setTimeout(resolve, poll));
    }
  }
  // Every way out of a landing runs release, so it leaves the queue too: a
  // waiter stopped while its session goes on would otherwise hold up every
  // worktree behind it.
  if (command === "release") {
    withQueue((queue) => ({ queue: without(queue, here) }));
    if (holder()?.path === here) {
      fs.rmSync(lock, { recursive: true, force: true });
      nudge();
    }
    return;
  }
  if (command === "leave") {
    withQueue((queue) => ({ queue: without(queue, here) }));
    return;
  }
  if (command === "holder") {
    const current = holder();
    if (current) console.log(current.path);
    return;
  }
  if (command === "queue") {
    const queue = withQueue((entries) => ({ result: entries }));
    const current = fs.existsSync(lock) && !gone(holder()) ? holder() : undefined;
    const lines = [...(current ? [["landing", current.path, current.since]] : []), ...queue.map((entry, at) => [ordinal(at + (current ? 2 : 1)), entry.path, entry.since])];
    lines.forEach((line) => console.log(line.join("\t")));
    return;
  }
  if (command === "move") {
    const [, , , target, way] = process.argv;
    if (!target || !["up", "down", "first"].includes(way)) {
      console.error("usage: land-lock.js move <path> up|down|first");
      process.exit(2);
    }
    let where = path.resolve(target);
    try {
      where = fs.realpathSync(where);
    } catch {}
    const moved = withQueue((queue) => {
      const at = queue.findIndex((entry) => entry.path === where);
      if (at === -1) return { result: false };
      const to = way === "first" ? 0 : way === "up" ? Math.max(0, at - 1) : Math.min(queue.length - 1, at + 1);
      const next = [...queue];
      const [entry] = next.splice(at, 1);
      next.splice(to, 0, entry);
      return { queue: next, result: true };
    });
    if (!moved) {
      console.error(`${where} isn't waiting in the landing queue`);
      process.exit(3);
    }
    return;
  }
  console.error("usage: land-lock.js take|wait|release|leave|holder|queue|move|phase [words]");
  process.exit(2);
};

main();
