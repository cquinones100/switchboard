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

const { encodePrefix, findSessions, formatRelativeTime, readLastUserMessage, readSessionCwd } = require("../bin/claude-here");

test("finds the target's sessions, newest first, with each one's own directory, and none of a sibling's or a nested worktree's", async () => {
  const projects = fs.mkdtempSync(path.join(os.tmpdir(), "claude-here-"));
  try {
    // Real folders, since a session whose folder is gone isn't offered.
    const work = fs.mkdtempSync(path.join(projects, "work-"));
    const session = (cwd, day) => {
      const id = crypto.randomUUID();
      const dir = path.join(projects, encodePrefix(cwd));
      fs.mkdirSync(dir, { recursive: true });
      fs.mkdirSync(cwd, { recursive: true });
      const file = path.join(dir, `${id}.jsonl`);
      fs.writeFileSync(file, `${JSON.stringify({ type: "user", cwd, sessionId: id })}\n`);
      fs.utimesSync(file, new Date(day), new Date(day));
      return id;
    };
    const oldest = session(`${work}/repo`, "2026-01-01");
    const nested = session(`${work}/repo/packages/app`, "2026-03-01");
    const middle = session(`${work}/repo`, "2026-02-01");
    session(`${work}/repo-other`, "2026-04-01");
    const worktree = session(`${work}/repo/.claude/worktrees/feature-x`, "2026-05-01");
    fs.writeFileSync(path.join(projects, encodePrefix(`${work}/repo`), "agent-a1b2c3.jsonl"), `${JSON.stringify({ cwd: `${work}/repo` })}\n`);
    // A transcript that can't be read: a link to nothing, by a session's name.
    fs.symlinkSync(path.join(work, "gone.jsonl"), path.join(projects, encodePrefix(`${work}/repo`), `${crypto.randomUUID()}.jsonl`));

    const found = await findSessions(`${work}/repo`, projects);
    assert.deepEqual(found.map((s) => s.id), [nested, middle, oldest], "a sibling under the same lossy name, a nested worktree, a subagent's transcript and one that can't be read left out");
    assert.equal(found[0].cwd, `${work}/repo/packages/app`, "each resumed where it ran");
    assert.deepEqual((await findSessions(`${work}/repo/.claude/worktrees/feature-x`, projects)).map((s) => s.id), [worktree], "the worktree's own, asked from there");
    assert.deepEqual(await findSessions(`${work}/none`, projects), []);
    fs.rmSync(`${work}/repo/packages/app`, { recursive: true });
    assert.deepEqual((await findSessions(`${work}/repo`, projects)).map((s) => s.id), [middle, oldest], "one whose folder is gone isn't offered");

    assert.deepEqual(await findSessions("/work/repo", path.join(projects, "missing")), []);
  } finally {
    fs.rmSync(projects, { recursive: true, force: true });
  }
});

test("reads a session's directory and the last thing you typed in it, skipping what Claude Code adds", async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "claude-here-"));
  try {
    const file = path.join(dir, "s.jsonl");
    fs.writeFileSync(file, [
      JSON.stringify({ type: "permission-mode", permissionMode: "auto" }),
      "not json at all",
      JSON.stringify({ type: "user", cwd: "/work/repo", message: { content: "first prompt" } }),
      JSON.stringify({ type: "user", cwd: "/somewhere/else", message: { content: [{ type: "text", text: "follow-up\n  \u001b[2J\u001b[Hquestion\u0007" }] } }),
      JSON.stringify({ type: "user", isMeta: true, message: { content: "meta caveat" } }),
      JSON.stringify({ type: "user", message: { content: [{ type: "tool_result", tool_use_id: "x", content: "output" }] } }),
      JSON.stringify({ type: "user", message: { content: "<command-name>/clear</command-name>" } }),
    ].join("\n"));
    assert.equal(await readSessionCwd(file), "/work/repo", "the first directory it names");
    assert.equal(await readLastUserMessage(file), "follow-up question", "terminal escapes and control characters dropped");

    fs.writeFileSync(file, `${JSON.stringify({ type: "permission-mode", permissionMode: "auto" })}\n`);
    assert.equal(await readSessionCwd(file), null);
    assert.equal(await readLastUserMessage(file), null);
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
