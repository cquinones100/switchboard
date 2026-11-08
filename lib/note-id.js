// A note's stable ID, which survives the note being renamed or moved, in the
// notes app or outside it (mv, Finder, git, a Claude editing the file): a
// last line `<!-- note-id: n-<12 hex> -->`, which Notes hides and keeps last
// (notes/lua/notes/id.lua reads and writes the same line). wtn's branch
// claims (bin/worktree-from-note), the work-a-note mode's state
// (lib/sidebar/autopilot.js) and the items it holds or skips name a note by
// it rather than by its path.

const crypto = require("node:crypto");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { execFileSync } = require("node:child_process");

const LINE = /^<!-- note-id: (n-[0-9a-f]{12}) -->$/;
// How much of a note's end is read for its ID first: the line and the blank
// lines and spaces an editor may leave after it. A note with lines added
// below its ID since (by a script, before Notes moved the ID last again) is
// read whole.
const TAIL = 300;

const notesDir = () => process.env.NOTES_DIR || path.join(os.homedir(), "Notes");
const newId = () => `n-${crypto.randomBytes(6).toString("hex")}`;
const lineOf = (id) => `<!-- note-id: ${id} -->`;

// The ID in a note's text, from its last line holding one.
const idIn = (text) => {
  const lines = String(text).split("\n");
  for (let at = lines.length - 1; at >= 0; at--) {
    const found = lines[at].trim().match(LINE);
    if (found) return found[1];
  }
  return "";
};

// The text without its ID line, for anything that shows or sends a note's
// words.
const withoutId = (text) => String(text).split("\n").filter((line) => !LINE.test(line.trim())).join("\n");

const tailOf = (file) => {
  let fd;
  try {
    fd = fs.openSync(file, "r");
    const size = fs.fstatSync(fd).size;
    const buffer = Buffer.alloc(Math.min(TAIL, size));
    fs.readSync(fd, buffer, 0, buffer.length, size - buffer.length);
    return buffer.toString("utf8");
  } catch {
    return "";
  } finally {
    if (fd !== undefined) fs.closeSync(fd);
  }
};

const idOf = (file) => {
  const id = idIn(tailOf(file));
  if (id) return id;
  try {
    return idIn(fs.readFileSync(file, "utf8"));
  } catch {
    return "";
  }
};

// Every note of the notes directory but its deleted ones and other
// dot-folders, as absolute paths.
const noteFiles = (dir = notesDir()) => {
  const walk = (folder) => {
    try {
      return fs.readdirSync(folder, { withFileTypes: true }).flatMap((entry) => {
        if (entry.name.startsWith(".")) return [];
        const full = path.join(folder, entry.name);
        if (entry.isDirectory()) return walk(full);
        return entry.isFile() && entry.name.endsWith(".md") ? [full] : [];
      });
    } catch {
      return [];
    }
  };
  return walk(dir);
};

// Each note's ID, kept against its modified time and its change time, so
// the daemon's checks read only the notes that changed: writeId keeps the
// modified time, but setting it changes the change time.
const known = new Map();
const cachedId = (file) => {
  let stat;
  try {
    stat = fs.statSync(file);
  } catch {
    return "";
  }
  const kept = known.get(file);
  if (kept && kept.mtime === stat.mtimeMs && kept.ctime === stat.ctimeMs) return kept.id;
  const id = idOf(file);
  known.set(file, { mtime: stat.mtimeMs, ctime: stat.ctimeMs, id });
  return id;
};

// The notes holding an ID, the one made first first: a copy of a note's file
// carries its ID until Notes or ensureId gives the copy one of its own, and
// the copy is the one changed later (or, changed at the same moment, the one
// whose path sorts later).
const holding = (id, dir = notesDir()) => {
  if (!id) return [];
  const mtime = (file) => known.get(file)?.mtime ?? 0;
  return noteFiles(dir)
    .filter((file) => cachedId(file) === id)
    .sort((a, b) => mtime(a) - mtime(b) || (a < b ? -1 : a > b ? 1 : 0));
};
const findById = (id, dir = notesDir()) => holding(id, dir)[0];

// Its modified time kept, so a note given an ID doesn't move up the notes
// list or read as just changed (the work-a-note mode waits a minute after a
// change). Notes' watch on the directory takes the line in, as it takes in
// any change made outside it.
const writeId = (file, text, id) => {
  const stat = fs.statSync(file);
  const kept = withoutId(text).replace(/\n*$/, "");
  fs.writeFileSync(file, `${kept}${kept ? "\n\n" : ""}${lineOf(id)}\n`);
  fs.utimesSync(file, stat.atime, stat.mtime);
  known.delete(file);
};

// A note given back an ID it lost, as when something outside Notes rewrote
// it without its last line.
const giveId = (file, id) => writeId(file, fs.readFileSync(file, "utf8"), id);

// The note's ID, given one when it has none, or a new one when it is a copy
// of another note that holds the same ID.
// Several wtn started at once (M-p's popup, several items picked) give one
// note its ID, or bring one repository's claims to IDs, one at a time, each reading it again under a lock in the
// temporary directory, so all of them claim by the one ID the note keeps.
// A lock older than LOCK_STALE_MS is a process that died holding it; past
// LOCK_WAIT_MS it goes on without one.
const LOCK_WAIT_MS = 5_000;
const LOCK_STALE_MS = 10_000;
const locked = (file, fn) => {
  const lock = path.join(os.tmpdir(), `note-id-${crypto.createHash("sha256").update(fs.realpathSync(file)).digest("hex").slice(0, 16)}.lock`);
  const start = Date.now();
  let held = false;
  while (!held && Date.now() - start < LOCK_WAIT_MS) {
    try {
      fs.writeFileSync(lock, String(process.pid), { flag: "wx" });
      held = true;
    } catch (err) {
      if (err.code !== "EEXIST") break;
      try {
        if (Date.now() - fs.statSync(lock).mtimeMs > LOCK_STALE_MS) fs.rmSync(lock, { force: true });
      } catch {}
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 50);
    }
  }
  try {
    return fn();
  } finally {
    if (held) fs.rmSync(lock, { force: true });
  }
};

const ensureId = (file, dir = notesDir()) => {
  const settled = () => {
    const text = fs.readFileSync(file, "utf8");
    const id = idIn(text);
    const first = id && holding(id, dir).map((f) => fs.realpathSync(f))[0];
    return { text, id: id && (!first || first === fs.realpathSync(file)) ? id : "" };
  };
  const now = settled();
  if (now.id) return now.id;
  return locked(file, () => {
    const { text, id } = settled();
    if (id) return id;
    const fresh = newId();
    writeId(file, text, fresh);
    return fresh;
  });
};

// The title of the note at a path: its first line that isn't blank, without
// Markdown's heading marks, as Notes reads it.
const titleOf = (text) =>
  String(text)
    .split("\n")
    .map((line) => line.replace(/^\s*#+\s*/, "").trim())
    .find((line) => line && !LINE.test(line)) ?? "";

// wtn's claims (branch.<branch>.wtnItem) from before notes had IDs read
// "<note's path in the notes directory>: <item's first line>". Once per
// repository (switchboard.wtnClaimsById marks it done), each is rewritten as
// "<note's ID>: <item's first line>", the note found at that path, else the
// one note titled as the path's name, else the one note holding the item,
// each only when it holds the item, ticked or not. Returns the claims it couldn't place, left as they were, which
// the picker and the mode still read by path.
const MIGRATED = "switchboard.wtnClaimsById";
const migrateClaims = (root, dir = notesDir()) => {
  const git = (...args) => execFileSync("git", ["-C", root, ...args], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
  const tryGit = (...args) => {
    try {
      return git(...args);
    } catch {
      return "";
    }
  };
  if (tryGit("config", "--get", MIGRATED) === "true") return [];
  // One wtn at a time per repository, several being started at once, each
  // looking at the marker again once it has the lock.
  return locked(root, () => {
    if (tryGit("config", "--get", MIGRATED) === "true") return [];
    const unplaced = [];
    const claims = tryGit("config", "--get-regexp", "^branch\\..*\\.wtnitem$").split("\n").filter(Boolean);
    let notes;
    const read = () =>
      (notes ??= noteFiles(dir).map((file) => {
        const text = fs.readFileSync(file, "utf8");
        return { file, title: titleOf(text), items: new Set(text.split("\n").map((line) => line.match(/^- \[[ xX]\] (.*\S.*)$/)?.[1].trim()).filter(Boolean)) };
      }));
    for (const row of claims) {
      const [key, ...rest] = row.split(" ");
      const value = rest.join(" ");
      if (/^n-[0-9a-f]{12}: /.test(value)) continue;
      const [, note, text] = value.match(/^(.*?\.md): (.*)$/s) ?? [];
      if (!note) {
        unplaced.push(value);
        continue;
      }
      // Only a note holding the item, ticked or not, so another note made
      // under a renamed note's old name or title isn't taken for it.
      const at = path.join(dir, note);
      const holders = read().filter((n) => n.items.has(text));
      const only = (list) => (list.length === 1 ? list[0].file : undefined);
      const file =
        holders.find((n) => fs.existsSync(at) && fs.realpathSync(n.file) === fs.realpathSync(at))?.file ||
        only(holders.filter((n) => n.title === path.basename(note, ".md"))) ||
        only(holders);
      if (!file) {
        unplaced.push(value);
        continue;
      }
      const branch = key.replace(/^branch\./, "").replace(/\.wtnitem$/, "");
      git("config", `branch.${branch}.wtnItem`, `${ensureId(file, dir)}: ${text}`);
    }
    tryGit("config", MIGRATED, "true");
    return unplaced;
  });
};

module.exports = { withoutId, idOf, ensureId, giveId, findById, titleOf, migrateClaims };
