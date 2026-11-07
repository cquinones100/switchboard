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

test("queue-and-clean ticks the note's item before its cleanup removes the worktree, after a PR, a direct landing and with no PR, so the work-a-note mode never sees the item free", () => {
  const text = fs.readFileSync(path.join(skillsDir, "queue-and-clean", "SKILL.md"), "utf8");
  const order = (line, ...steps) => {
    const at = steps.map((step) => line.indexOf(step));
    assert.ok(at.every((i) => i >= 0) && at.every((i, n) => n === 0 || i > at[n - 1]), `${steps.join(", then ")} in: ${line.slice(0, 120)}`);
  };
  const line = (start) => text.split("\n").find((l) => l.startsWith(start));
  for (const start of ["- `deploy-test: approved", "14. **Then as work already in the default branch**", "2. **When it is**"]) order(line(start), '"Cleanup"', '"Closing out"', "before the worktree goes");
  order(text.split("## Cleanup")[1], "step 2 has found nothing would be lost", '"Closing out"', "clean-worktree: success");
  assert.match(text, /## Closing out\n\nInside the cleanup, once clean-worktree's step 2 has found nothing would be lost and before its step 3 removes the worktree/);
});

// Switchboard's project skills, in .claude/skills: for working on Switchboard
// itself, so setup.sh never links them (test/setup.test.js checks that).
const projectDir = path.join(__dirname, "../.claude/skills");
const projectSkills = fs.readdirSync(projectDir).filter((name) => fs.existsSync(path.join(projectDir, name, "SKILL.md")));

test("every project skill has frontmatter naming it after its folder, with a description", () => {
  assert.ok(projectSkills.includes("plan-item"));
  for (const name of projectSkills) {
    const text = fs.readFileSync(path.join(projectDir, name, "SKILL.md"), "utf8");
    const frontmatter = text.match(/^---\n([\s\S]*?)\n---\n/);
    assert.ok(frontmatter, `${name} starts with its frontmatter`);
    assert.match(frontmatter[1], new RegExp(`^name: ${name}$`, "m"), `${name}'s name is its folder's`);
    assert.match(frontmatter[1], /^description: \S/m, `${name} has a description`);
  }
});

test("plan-item asks where the item goes: the top, in order of difficulty, or the bottom", () => {
  const text = fs.readFileSync(path.join(projectDir, "plan-item/SKILL.md"), "utf8");
  assert.match(text, /^disable-model-invocation: true$/m, "it reads your note, so only you start it");
  for (const option of ["**Top of the list**", "**In order of difficulty**", "**Bottom of the list**"]) assert.ok(text.includes(option), option);
});
