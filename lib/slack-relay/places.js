// Part of bin/slack-relay; see the comment at the top of that file.

const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { tryTmux } = require("./util");
const { inReviewDir, isUntrusted } = require("../untrusted");

// ── Places ──────────────────────────────────────────────────────────────────

// Where `new <place>: <prompt>` starts its Claude. A place is a directory
// (a path starting ~ or /), or a name looked up among the git repositories
// you work in, found the way bin/wt-revive finds them for M-o: every
// open tab's, and every one a Claude session has run in over the last
// RECENT_DAYS. A repository's own name picks its main checkout; otherwise the
// name is matched against each worktree's branch and directory, so a ticket
// ID finds its worktree. Exactly one match, or nothing is started.
const RECENT_DAYS = 14;

const git = (dir, ...args) => {
  try {
    return execFileSync("git", ["-C", dir, ...args], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
  } catch {
    return undefined;
  }
};

// Directories Claude sessions have run in since `since` (by default the last
// RECENT_DAYS), as { dir, at }, the time of the session's last activity: the
// last cwd in each transcript, read from its end, since some are megabytes
// long.
const sessionDirs = (since = Date.now() - RECENT_DAYS * 24 * 60 * 60_000) => {
  const projects = path.join(os.homedir(), ".claude/projects");
  let dirs = [];
  try {
    dirs = fs.readdirSync(projects);
  } catch {
    return [];
  }
  return dirs.flatMap((dir) => {
    let files = [];
    try {
      files = fs.readdirSync(path.join(projects, dir)).filter((name) => name.endsWith(".jsonl"));
    } catch {
      return [];
    }
    return files.flatMap((name) => {
      const file = path.join(projects, dir, name);
      try {
        const { mtimeMs, size } = fs.statSync(file);
        if (mtimeMs < since) return [];
        const fd = fs.openSync(file, "r");
        const length = Math.min(size, 65_536);
        const buffer = Buffer.alloc(length);
        fs.readSync(fd, buffer, 0, length, size - length);
        fs.closeSync(fd);
        const cwd = [...buffer.toString("utf8").matchAll(/"cwd":"([^"]*)"/g)].at(-1)?.[1];
        return cwd ? [{ dir: cwd, at: mtimeMs }] : [];
      } catch {
        return [];
      }
    });
  });
};

// Every worktree of every repository in use, as { path, branch, repo, main }.
const places = () => {
  const panes = tryTmux(["list-panes", "-a", "-F", "#{pane_current_path}"])?.split("\n") ?? [];
  const roots = new Set(
    [...new Set([...panes, ...sessionDirs().map((session) => session.dir)])].flatMap((dir) => {
      if (!fs.existsSync(dir)) return [];
      const common = git(dir, "rev-parse", "--path-format=absolute", "--git-common-dir");
      // A bare repository has no checkout of its own.
      return common && path.basename(common) === ".git" ? [path.dirname(common)] : [];
    }),
  );
  return [...roots].flatMap((repo) =>
    (git(repo, "worktree", "list", "--porcelain") ?? "")
      .split("\n\n")
      .filter((block) => block.trim() && !/^prunable/m.test(block) && !/^bare$/m.test(block))
      // Never someone else's PR, which no Claude may be started in.
      .filter((block) => !inReviewDir(/^worktree (.*)$/m.exec(block)?.[1]))
      .map((block) => {
        const tree = block.match(/^worktree (.+)$/m)?.[1];
        const branch = block.match(/^branch refs\/heads\/(.+)$/m)?.[1] ?? "(detached)";
        return { path: tree, branch, repo: path.basename(repo), main: tree === repo };
      })
      .filter((place) => place.path),
  );
};

const home = (dir) => dir.replace(/^~(?=\/|$)/, os.homedir());
const shortPath = (dir) => (dir.startsWith(os.homedir()) ? `~${dir.slice(os.homedir().length)}` : dir);
const describe = (place) => `\`${shortPath(place.path)}\` (${place.repo}, ${place.branch})`;

// { dir, said } for a place, or { error } saying why not.
const resolvePlace = (spec) => {
  if (/^[~/]/.test(spec)) {
    const dir = path.resolve(home(spec));
    if (!fs.existsSync(dir) || !fs.statSync(dir).isDirectory()) return { error: `There's no directory \`${spec}\`, so I didn't start anything.` };
    // Never someone else's PR, whose project settings and hooks Claude would load.
    if (isUntrusted(dir)) return { error: `\`${spec}\` holds someone else's PR, checked out untrusted, so I didn't start anything there.` };
    return { dir, said: `\`${shortPath(dir)}\`` };
  }
  const all = places();
  const wanted = spec.toLowerCase();
  const repo = all.filter((place) => place.main && place.repo.toLowerCase() === wanted);
  const matches = repo.length
    ? repo
    : all.filter((place) => place.branch.toLowerCase().includes(wanted) || path.basename(place.path).toLowerCase().includes(wanted));
  if (matches.length === 1) {
    // Never someone else's PR, wherever its worktree was moved.
    if (isUntrusted(matches[0].path)) return { error: `\`${spec}\` is someone else's PR, checked out untrusted, so I didn't start anything there.` };
    return { dir: matches[0].path, said: describe(matches[0]) };
  }
  if (!matches.length) {
    return { error: `Nothing matches \`${spec}\`: no repository by that name, and no worktree whose branch or directory contains it. Nothing started.` };
  }
  return {
    error:
      `\`${spec}\` matches ${matches.length} places, so I didn't start anything. Name one more closely:\n` +
      matches.slice(0, 10).map((place) => `• ${describe(place)}`).join("\n") +
      (matches.length > 10 ? `\n…and ${matches.length - 10} more` : ""),
  };
};

// The New tab form's menu of places: every repository checkout a Claude
// session has ever run in, or an open tab is in, at its top level (a session
// in a subdirectory counts for its checkout), and every directory outside a
// repository, so long as it still exists. Most recently used first, grouped by
// repository (its main checkout's name), a worktree named by its own
// directory, with directories outside any repository last. At most
// MAX_PLACES, the most a Slack menu holds. Asked of git once a directory, and
// only when the form opens.
const MAX_PLACES = 100;
// Slack refuses an option whose value is longer.
const MAX_VALUE = 150;
const placeChoices = () => {
  const panes = (tryTmux(["list-panes", "-a", "-F", "#{pane_current_path}"])?.split("\n") ?? []).filter(Boolean).map((dir) => ({ dir, at: Date.now() }));
  const latest = new Map();
  [...panes, ...sessionDirs(0)].forEach(({ dir, at }) => latest.set(dir, Math.max(at, latest.get(dir) ?? 0)));
  const tops = new Map();
  [...latest.entries()]
    .filter(([dir]) => fs.existsSync(dir) && fs.statSync(dir).isDirectory())
    .forEach(([dir, at]) => {
      const [top, common] = (git(dir, "rev-parse", "--path-format=absolute", "--show-toplevel", "--git-common-dir") ?? "").split("\n");
      const repo = common && path.basename(common) === ".git" ? path.dirname(common) : undefined;
      const place = repo && top ? top : dir;
      const known = tops.get(place);
      if (!known || known.at < at) tops.set(place, { dir: place, repo, at });
    });
  const choices = [...tops.values()]
    .sort((a, b) => b.at - a.at)
    .slice(0, MAX_PLACES)
    .map(({ dir, repo, at }) => ({
      group: repo ? path.basename(repo) : undefined,
      label: !repo ? shortPath(dir) : dir === repo ? path.basename(repo) : `⎇ ${path.basename(dir)}`,
      value: shortPath(dir),
      at,
    }))
    .filter((choice) => choice.value.length <= MAX_VALUE);
  const order = [...new Set(choices.map((choice) => choice.group))].sort((a, b) => (a === undefined) - (b === undefined));
  return order.map((group) => ({ group: group ?? "Other directories", choices: choices.filter((choice) => choice.group === group) }));
};

module.exports = { places, resolvePlace, shortPath, MAX_PLACES, placeChoices };
