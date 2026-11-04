// Tests for bin/claude-here, which resumes the Claude session of the current
// worktree. Run with: node --test test/*.test.js
//
// Sessions are made in a temporary projects directory, laid out as Claude
// Code's ~/.claude/projects is, so nothing reads your own.

const { test } = require("node:test");
const assert = require("node:assert/strict");
const crypto = require("node:crypto");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const { encodePrefix, findSessions, formatRelativeTime, readSessionTail } = require("../bin/claude-here");

test("finds the target's sessions, last active first, with each one's own directory, and none of a sibling's or a nested worktree's", async () => {
  const projects = fs.mkdtempSync(path.join(os.tmpdir(), "claude-here-"));
  try {
    // Real folders, since a session whose folder is gone isn't offered.
    const work = fs.mkdtempSync(path.join(projects, "work-"));
    // Each one opened again after it was last used, as tmux-resurrect does
    // after a restart, the oldest last: Claude Code appends entries with no
    // timestamp then, so the file's modified time says nothing.
    let opened = Date.parse("2026-12-31");
    const session = (cwd, day, { startedIn = cwd } = {}) => {
      const id = crypto.randomUUID();
      const dir = path.join(projects, encodePrefix(startedIn));
      fs.mkdirSync(dir, { recursive: true });
      fs.mkdirSync(cwd, { recursive: true });
      fs.mkdirSync(startedIn, { recursive: true });
      const file = path.join(dir, `${id}.jsonl`);
      fs.writeFileSync(file, [
        JSON.stringify({ type: "user", cwd: startedIn, sessionId: id, timestamp: "2025-12-01T00:00:00Z" }),
        JSON.stringify({ type: "user", cwd, sessionId: id, timestamp: `${day}T00:00:00Z` }),
        JSON.stringify({ type: "cost-state" }),
      ].join("\n"));
      opened -= 60 * 1000;
      fs.utimesSync(file, new Date(opened), new Date(opened));
      return id;
    };
    const oldest = session(`${work}/repo`, "2026-01-01");
    const nested = session(`${work}/repo/packages/app`, "2026-03-01");
    const middle = session(`${work}/repo`, "2026-02-01");
    // Started in a folder of the repository, then moved up to its top: resumed where it started.
    const movedUp = session(`${work}/repo`, "2026-02-20", { startedIn: `${work}/repo/packages/web` });
    // Started in the repository under its old name, and in a folder of it
    // since, so resumed from the repository's.
    const renamed = session(`${work}/repo/lib`, "2026-02-15", { startedIn: `${work}/repo` });
    fs.writeFileSync(path.join(projects, encodePrefix(`${work}/repo`), `${renamed}.jsonl`), [
      // Renamed from a name that encodes the same, which is gone now.
      JSON.stringify({ type: "user", cwd: `${work}.repo`, timestamp: "2025-12-01T00:00:00Z" }),
      JSON.stringify({ type: "user", cwd: `${work}/repo/lib`, timestamp: "2026-02-15T00:00:00Z" }),
    ].join("\n"));
    session(`${work}/repo-other`, "2026-04-01");
    const worktree = session(`${work}/repo/.claude/worktrees/feature-x`, "2026-05-01");
    // Started in the repository, then moved into one of its worktrees: that worktree's now.
    session(`${work}/repo/.claude/worktrees/feature-y`, "2026-06-01", { startedIn: `${work}/repo` });
    fs.writeFileSync(path.join(projects, encodePrefix(`${work}/repo`), "agent-a1b2c3.jsonl"), `${JSON.stringify({ cwd: `${work}/repo` })}\n`);
    // A transcript that can't be read: a link to nothing, by a session's name.
    fs.symlinkSync(path.join(work, "gone.jsonl"), path.join(projects, encodePrefix(`${work}/repo`), `${crypto.randomUUID()}.jsonl`));

    const found = await findSessions(`${work}/repo`, projects);
    assert.deepEqual(found.map((s) => s.id), [nested, movedUp, renamed, middle, oldest], "a sibling under the same lossy name, a nested worktree, one that moved into one, a subagent's transcript and one that can't be read left out");
    assert.equal(found[0].cwd, `${work}/repo/packages/app`, "each resumed where it ran");
    assert.equal(found[1].cwd, `${work}/repo/packages/web`, "one that moved up resumed where it started");
    assert.equal(found[2].cwd, `${work}/repo`, "one whose starting folder was renamed resumed where its transcript is");
    assert.equal(found[0].lastActiveMs, Date.parse("2026-03-01"), "active when its last timestamp says");
    assert.deepEqual((await findSessions(`${work}/repo/.claude/worktrees/feature-x`, projects)).map((s) => s.id), [worktree], "the worktree's own, asked from there");
    assert.deepEqual(await findSessions(`${work}/none`, projects), []);
    fs.rmSync(`${work}/repo/packages/app`, { recursive: true });
    assert.deepEqual((await findSessions(`${work}/repo`, projects)).map((s) => s.id), [movedUp, renamed, middle, oldest], "one whose folder is gone isn't offered");

    assert.deepEqual(await findSessions("/work/repo", path.join(projects, "missing")), []);
  } finally {
    fs.rmSync(projects, { recursive: true, force: true });
  }
});

test("reads a session's last directory, last activity and the last thing you typed in it, skipping what Claude Code adds", async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "claude-here-"));
  try {
    const file = path.join(dir, "s.jsonl");
    fs.writeFileSync(file, [
      JSON.stringify({ type: "permission-mode", permissionMode: "auto" }),
      "not json at all",
      JSON.stringify({ type: "user", cwd: "/work/repo", timestamp: "2026-06-01T10:00:00Z", message: { content: "first prompt" } }),
      JSON.stringify({ type: "user", cwd: "/work/repo/lib", timestamp: "2026-06-01T11:00:00Z", message: { content: [{ type: "text", text: "follow-up\n  \u001b[2J\u001b[Hquestion\u0007" }] } }),
      JSON.stringify({ type: "user", isMeta: true, message: { content: "meta caveat" } }),
      JSON.stringify({ type: "user", message: { content: [{ type: "tool_result", tool_use_id: "x", content: "output" }] } }),
      JSON.stringify({ type: "user", message: { content: "<command-name>/clear</command-name>" } }),
      JSON.stringify({ type: "cost-state" }),
    ].join("\n"));
    assert.deepEqual(await readSessionTail(file), {
      cwd: "/work/repo/lib",
      lastActiveMs: Date.parse("2026-06-01T11:00:00Z"),
      lastMessage: "follow-up question",
    }, "the last directory and timestamp, terminal escapes and control characters dropped");

    // Far enough back to need more than the first piece read from the end.
    fs.appendFileSync(file, `\n${JSON.stringify({ type: "cost-state", pad: "x".repeat(300 * 1024) })}`);
    assert.equal((await readSessionTail(file)).lastMessage, "follow-up question", "found past the first piece read");

    fs.writeFileSync(file, `${JSON.stringify({ type: "permission-mode", permissionMode: "auto" })}\n`);
    assert.deepEqual(await readSessionTail(file), { cwd: null, lastActiveMs: null, lastMessage: null });
    assert.deepEqual(await readSessionTail(path.join(dir, "missing.jsonl")), { cwd: null, lastActiveMs: null, lastMessage: null });
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("says how long ago a session was active", () => {
  const now = new Date("2026-06-12T12:00:00Z").getTime();
  const minute = 60 * 1000;
  assert.deepEqual(
    [0.5, 5, 180, 1800, 4 * 1440].map((minutes) => formatRelativeTime(now - minutes * minute, now)),
    ["just now", "5m ago", "3h ago", "yesterday", "4d ago"],
  );
  assert.equal(formatRelativeTime(now - 30 * 1440 * minute, now), new Date(now - 30 * 1440 * minute).toLocaleDateString());
});
