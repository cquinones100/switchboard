#!/usr/bin/env node
// The landing lock of queue-and-clean's "Land on the default branch": one
// worktree of a repository lands on its default branch at a time, so two
// never rebase onto the same one and push over each other. It lives in the
// main clone's git directory, shared by every worktree of it.
//
//   land-lock.js take      takes it for this worktree: exit 0 taken (or held
//                          by this worktree already, so a landing checks it
//                          still holds it after a wait), 3 held by another,
//                          which it prints
//   land-lock.js wait      takes it, waiting as long as another holds it
//   land-lock.js release   lets it go, when this worktree holds it
//   land-lock.js holder    prints which worktree holds it, nothing when none
//
// A landing can wait on you for as long as you like (its push question), so
// the lock never expires with time: it is taken over only from a landing that
// is gone, its worktree removed or no Claude session running in it.

const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");
const { liveSessionsIn } = require("../../../../lib/worktree");

const git = (...args) => execFileSync("git", args, { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();

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
  console.error("usage: land-lock.js take|wait|release|holder");
  process.exit(2);
};

main();
