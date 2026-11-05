// What the scripts that open a worktree for a piece of work share
// (bin/worktree-from-ticket, bin/worktree-from-note): the branch's slug, the
// main clone, the local settings linked in, and the worktree made or reused,
// refused while Claude already works in it. Errors are thrown, for each
// script's own `fail` to print.

const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const slugify = (text) =>
  text
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9\s-]/g, "")
    .replace(/\s+/g, "-")
    .replace(/-+/g, "-")
    .replace(/^-|-$/g, "");

// Two helpers because execFileSync returns null when stdout isn't piped, so a
// single trim()-ing wrapper would throw on the inherit-stdio calls.
const gitOutput = (args) => execFileSync("git", args, { encoding: "utf8" }).trim();

const gitRun = (args, options = {}) => execFileSync("git", args, { stdio: "ignore", ...options });

// --git-common-dir rather than --show-toplevel: inside an existing worktree the
// latter returns that worktree, and new worktrees belong beside the main repo.
const repoRoot = () => {
  try {
    return path.resolve(gitOutput(["rev-parse", "--git-common-dir"]), "..");
  } catch {
    throw new Error("not inside a git repository");
  }
};

// Claude Code reads .claude/settings.local.json from the worktree root, not the
// main clone, and git never copies it because it is gitignored. Symlinking the
// main clone's file keeps one source of truth for personal permissions and the
// auto mode environment profile. Skipped when the worktree already has its own.
const linkLocalSettings = (root, worktreePath) => {
  const source = path.join(root, ".claude", "settings.local.json");
  const target = path.join(worktreePath, ".claude", "settings.local.json");
  if (!fs.existsSync(source) || fs.existsSync(target)) return;
  fs.mkdirSync(path.dirname(target), { recursive: true });
  fs.symlinkSync(source, target);
};

const CLAUDE_DIR = path.join(os.homedir(), ".claude");

const isAlive = (pid) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch (err) {
    // EPERM means it exists but belongs to someone else, which still counts.
    return err.code === "EPERM";
  }
};

// The title the session picker shows: a name given with /rename, else the
// title Claude Code generated from the first prompt. Both live in files Claude
// Code does not document, so anything unreadable just leaves the title blank.
const sessionTitle = (session) => {
  if (session.name && session.nameSource && session.nameSource !== "derived") {
    return session.name;
  }
  const projects = path.join(CLAUDE_DIR, "projects");
  const transcript = fs.existsSync(projects)
    ? fs
        .readdirSync(projects)
        .map((dir) => path.join(projects, dir, `${session.sessionId}.jsonl`))
        .find((file) => fs.existsSync(file))
    : undefined;
  if (!transcript) return session.name ?? "";
  const titles = fs
    .readFileSync(transcript, "utf8")
    .split("\n")
    .filter((line) => line.includes('"type":"ai-title"'))
    .map((line) => {
      try {
        return JSON.parse(line).aiTitle;
      } catch {
        return undefined;
      }
    })
    .filter(Boolean);
  return titles.at(-1) ?? session.name ?? "";
};

// Where the session's pane is, as tmux numbers it, so you can go straight to
// it. Blank outside tmux or when the pane has gone.
const tmuxWindow = (session) => {
  const pane = session.tmux?.match(/(%\d+)$/)?.[1];
  if (!pane) return "";
  try {
    return execFileSync("tmux", ["display-message", "-p", "-t", pane, "#{session_name}:#{window_index}"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
  } catch {
    return "";
  }
};

// Claude Code writes ~/.claude/sessions/<pid>.json for every running session,
// naming its working directory. A file whose process is gone is left over from
// a session that ended without cleaning up, and is ignored.
const liveSessionsIn = (worktreePath) => {
  const dir = path.join(CLAUDE_DIR, "sessions");
  if (!fs.existsSync(dir)) return [];
  return fs
    .readdirSync(dir)
    .filter((file) => file.endsWith(".json"))
    .map((file) => {
      try {
        return JSON.parse(fs.readFileSync(path.join(dir, file), "utf8"));
      } catch {
        return undefined;
      }
    })
    .filter(
      (session) =>
        session?.sessionId &&
        session.cwd &&
        (session.cwd === worktreePath || session.cwd.startsWith(`${worktreePath}/`)) &&
        isAlive(session.pid),
    );
};

// Idempotent so rerunning on the same work reopens the worktree instead of
// erroring: reuse the directory if it exists, and the branch if it exists.
// `work` names what the worktree is for ("ticket", "item") and `command` the
// function that opens it ("wt", "wtn"), for the refusal's words; `start`, the
// commit a new branch is made from, with no upstream, else the current HEAD.
const ensureWorktree = (worktreePath, branch, { work, command, start }) => {
  if (fs.existsSync(worktreePath)) {
    // Two sessions on one piece of work edit the same files and push the same
    // branch, so a worktree that already has Claude running in it is left to
    // that one.
    const sessions = liveSessionsIn(worktreePath);
    if (sessions.length) {
      const lines = sessions.map((session) => {
        const title = sessionTitle(session);
        const where = tmuxWindow(session);
        return `  Session: ${session.sessionId}\n` + `  Title:   ${title || "(none yet)"}` + (where ? `\n  Window:  ${where}` : "");
      });
      throw new Error(
        `Claude is already working on this ${work}, so ${command} will not start another session.\n` +
          lines.join("\n\n") +
          `\nSwitch to that session, or end it and run ${command} again.`,
      );
    }
    // A directory of the same name on another branch is another piece of
    // work, never taken for this one.
    const checkedOut = (() => {
      try {
        return gitOutput(["-C", worktreePath, "branch", "--show-current"]);
      } catch {
        return "";
      }
    })();
    if (checkedOut !== branch) {
      throw new Error(`${worktreePath} is there already, on ${checkedOut ? `branch ${checkedOut}` : "no branch of this repository"}, not ${branch}.`);
    }
    console.error("Worktree already exists, reusing it.");
    return;
  }

  const branchExists = (() => {
    try {
      gitRun(["show-ref", "--verify", "--quiet", `refs/heads/${branch}`]);
      return true;
    } catch {
      return false;
    }
  })();

  const args = branchExists
    ? ["worktree", "add", worktreePath, branch]
    : start
      ? ["worktree", "add", "--no-track", "-b", branch, worktreePath, start]
      : ["worktree", "add", "-b", branch, worktreePath];

  try {
    // git's stdout to fd 2 so it can't pollute anything a caller parses.
    gitRun(args, { stdio: ["inherit", 2, "inherit"] });
  } catch {
    throw new Error("git worktree creation failed (see output above)");
  }
};

module.exports = { slugify, gitOutput, gitRun, repoRoot, linkLocalSettings, liveSessionsIn, ensureWorktree };
