// Tests for how Switchboard's skills defer to a repository's own skill of the
// same name: every skill's opening step, hand-offs between skills, and
// bin/skill-path, which both use. Run with: node --test test/*.test.js

const { test } = require("node:test");
const assert = require("node:assert/strict");
const { execFileSync, spawnSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const skillsDir = path.join(__dirname, "../claude/skills");
const script = path.join(__dirname, "../bin/skill-path");
const skills = fs.readdirSync(skillsDir).filter((name) => fs.existsSync(path.join(skillsDir, name, "SKILL.md")));

// Hand-offs that need Switchboard's own text rather than the repository's copy.
const fixedPathExceptions = ["`~/switchboard/claude/skills/ci-loop/SKILL.md`, which a repository's copy leaves out"];

test("every skill opens by running skill-path with its own name, right after its frontmatter and title", () => {
  for (const name of skills) {
    const text = fs.readFileSync(path.join(skillsDir, name, "SKILL.md"), "utf8");
    const opening = `This is Switchboard's personal copy, installed for repositories that have none. Before anything else, run \`skill-path ${name}\`: if it prints a path other than this file's, read that file and follow it instead, with the same arguments, and stop reading here.`;
    assert.match(text, /^---\n[\s\S]*?\n---\n\n# [^\n]+\n\n/, `${name} starts with its frontmatter and title`);
    const afterTitle = text.replace(/^---\n[\s\S]*?\n---\n\n# [^\n]+\n\n/, "");
    assert.ok(afterTitle.startsWith(opening + "\n"), `${name}'s first paragraph is the opening step`);
  }
});

test("no skill names another skill's SKILL.md by a fixed path, but for the listed exceptions", () => {
  for (const name of skills) {
    let text = fs.readFileSync(path.join(skillsDir, name, "SKILL.md"), "utf8");
    for (const exception of fixedPathExceptions) text = text.replace(exception, "");
    const fixed = text.match(/[^\s`(]*skills\/[a-z-]+\/SKILL\.md/g) || [];
    assert.deepEqual(fixed, [], `${name} reads other skills through skill-path`);
  }
});

test("skill-path finds the worktree's copy, then the main clone's, then the personal one", () => {
  const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "skill-path-")));
  const env = { ...process.env, HOME: tmp, GIT_CONFIG_GLOBAL: "/dev/null", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e" };
  const run = (dir) => spawnSync(process.execPath, [script, "demo", dir], { env, encoding: "utf8" });
  const skill = (root) => {
    const file = path.join(root, ".claude/skills/demo/SKILL.md");
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, "# demo\n");
    return file;
  };
  try {
    const repo = path.join(tmp, "repo");
    const worktree = path.join(tmp, "wt");
    const outside = path.join(tmp, "outside");
    fs.mkdirSync(outside);
    execFileSync("git", ["init", "-q", "-b", "main", repo], { env });
    execFileSync("git", ["-C", repo, "commit", "-q", "--allow-empty", "-m", "start"], { env });
    execFileSync("git", ["-C", repo, "worktree", "add", "-q", "-b", "feature", worktree], { env });

    let result = run(worktree);
    assert.equal(result.status, 1, "none anywhere");
    assert.equal(result.stdout, "");

    const personal = skill(tmp);
    assert.equal(run(worktree).stdout.trim(), personal, "the personal one when the repository has none");
    assert.equal(run(outside).stdout.trim(), personal, "the personal one outside a repository");

    const main = skill(repo);
    assert.equal(run(worktree).stdout.trim(), main, "the main clone's when the worktree has none");
    assert.equal(run(path.join(repo, ".claude")).stdout.trim(), main, "from inside the main clone");

    const own = skill(worktree);
    assert.equal(run(worktree).stdout.trim(), own, "the worktree's own first");
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});
