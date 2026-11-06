#!/usr/bin/env node
// The landing lock of queue-and-clean's "Land on the default branch": one
// worktree of a repository lands on its default branch at a time, so two
// never rebase onto the same one and push over each other. Each tests and
// reviews before taking it. It lives in the
// main clone's git directory, shared by every worktree of it.
//
//   land-lock.js take      takes it for this worktree: exit 0 taken (or held
//                          by this worktree already, so a landing checks it
//                          still holds it after a wait), 3 held by another,
//                          which it prints
//   land-lock.js wait      takes it, waiting as long as another holds it
//   land-lock.js release   lets it go, when this worktree holds it
//   land-lock.js holder    prints which worktree holds it, nothing when none
//   land-lock.js phase [words]
//                          shows the landing's step in this tab, the pane's
//                          @agent_phase, its loop marked queue-and-clean's
//                          (@agent_loop), since a session following the skill
//                          by reading it never invokes it; no words clear it
//
// A landing holds it only to rebase, test and push, but one whose change no
// longer matches its review reviews again and asks again while holding it,
// which can wait on you for as long as you like, so the lock never expires
// with time: it is taken over only from a landing that
// is gone, its worktree removed or no Claude session running in it.

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

const holder = () => {
  try {
    return JSON.parse(fs.readFileSync(holderFile, "utf8"));
  } catch {
    return undefined;
  }
};

const hold = () => fs.writeFileSync(holderFile, JSON.stringify({ path: here, since: Date.now() }));

const gone = (current) => !current || !fs.existsSync(current.path) || liveSessionsIn(current.path).length === 0;

const take = (quiet) => {
  try {
    fs.mkdirSync(lock);
    hold();
    return 0;
  } catch (err) {
    if (err.code !== "EEXIST") throw err;
  }
  const current = holder();
  if (current?.path === here) return 0;
  if (gone(current)) {
    hold();
    return 0;
  }
  if (!quiet) console.log(`held by ${current.path} since ${new Date(current.since).toTimeString().slice(0, 5)}`);
  return 3;
};

const main = async () => {
  const command = process.argv[2];
  if (command === "take") process.exit(take());
  if (command === "wait") {
    const poll = Number(process.env.LAND_LOCK_POLL ?? 15) * 1000;
    while (take(true) !== 0) await new Promise((resolve) => setTimeout(resolve, poll));
    return;
  }
  if (command === "release") {
    if (holder()?.path === here) fs.rmSync(lock, { recursive: true, force: true });
    return;
  }
  if (command === "holder") {
    const current = holder();
    if (current) console.log(current.path);
    return;
  }
  console.error("usage: land-lock.js take|wait|release|holder|phase [words]");
  process.exit(2);
};

main();
