// Tests for setup.sh's parts, which must be safe to run again. Run with:
// node --test test/*.test.js
//
// A temporary HOME whose ~/editor-configs, the name before the rename, links
// to this clone, as an older clone's would: setup.sh links ~/switchboard to
// it, and the links land there and never in your own home. The tmux part (which fetches the
// plugins) and the git part (which sets this clone's own config) are left out.

const { test } = require("node:test");
const assert = require("node:assert/strict");
const { spawn, spawnSync, execFileSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const repo = path.join(__dirname, "..");
const script = path.join(repo, "setup.sh");

test("setup.sh links its parts once, moves a real file aside once, and changes nothing the second time", () => {
  const home = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "setup-test-")));
  const env = { ...process.env, HOME: home };
  const parts = ["lazygit", "neovim", "ghostty", "pgcli", "bin", "claude"];
  try {
    fs.symlinkSync(repo, path.join(home, "editor-configs"));
    fs.mkdirSync(path.join(home, ".config/editor-configs"), { recursive: true });
    fs.writeFileSync(path.join(home, ".config/editor-configs/settings.json"), JSON.stringify({ tips: false }));
    fs.mkdirSync(path.join(home, ".config/ghostty"), { recursive: true });
    fs.writeFileSync(path.join(home, ".config/ghostty/config"), "theme = mine\n");

    const first = spawnSync("bash", [script, ...parts], { env, encoding: "utf8" });
    assert.equal(first.status, 0, first.stdout + first.stderr);
    assert.match(first.stdout, /ghostty:\n  move .*\.config\/ghostty\/config aside to .*\.bak\.\d+\n  link .*\.config\/ghostty\/config/);
    assert.match(first.stdout, /bin:\n(  link .*\n)*  link .*\.local\/bin\/notes\n/);
    assert.equal(fs.readlinkSync(path.join(home, ".config/nvim")), path.join(home, "switchboard/nvim"));
    assert.equal(fs.realpathSync(path.join(home, "switchboard")), fs.realpathSync(repo), "~/switchboard leads to the clone");
    assert.deepEqual(JSON.parse(fs.readFileSync(path.join(home, ".config/switchboard/settings.json"), "utf8")), { tips: false }, "and the settings moved to their new folder");

    const check = spawnSync("bash", [script, "--check", ...parts], { env, encoding: "utf8" });
    assert.equal(check.stdout, parts.map((p) => `${p}: up to date\n`).join(""), "--check finds nothing left to do");

    const again = spawnSync("bash", [script, ...parts], { env, encoding: "utf8" });
    assert.equal(again.status, 0, again.stderr);
    assert.equal(again.stdout, parts.map((p) => `${p}: up to date\n`).join(""), "a second run changes nothing");
    assert.equal(fs.readdirSync(path.join(home, ".config/ghostty")).filter((f) => f.includes(".bak.")).length, 1, "and moves nothing aside again");

    fs.unlinkSync(path.join(home, ".config/nvim"));
    const pending = spawnSync("bash", [script, "--check", "neovim"], { env, encoding: "utf8" });
    assert.match(pending.stdout, /neovim:\n  would: link .*\.config\/nvim/);
    assert.equal(fs.existsSync(path.join(home, ".config/nvim")), false, "--check links nothing");

    const unknown = spawnSync("bash", [script, "nonsense"], { env, encoding: "utf8" });
    assert.equal(unknown.status, 1);
    assert.match(unknown.stderr, /Unknown part: nonsense/);
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test("setup.sh claude merges Switchboard's settings into yours once, keeping yours, and makes a linked settings file your own", () => {
  const home = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "setup-claude-")));
  const env = { ...process.env, HOME: home };
  const product = JSON.parse(fs.readFileSync(path.join(repo, "claude/switchboard-settings.json"), "utf8"));
  try {
    fs.symlinkSync(repo, path.join(home, "switchboard"));
    fs.mkdirSync(path.join(home, ".claude"));
    // Yours: a model, a permission, a hook of your own beside an older copy of
    // Switchboard's, and a statusline of your own.
    const yours = {
      model: "sonnet",
      permissions: { allow: ["Bash(make *)", "Bash(git log *)"] },
      statusLine: { type: "command", command: "my-statusline" },
      hooks: { Stop: [{ hooks: [{ type: "command", command: "say done" }, { type: "command", command: "bash ~/.config/tmux/agent-state.sh set done" }] }] },
    };
    fs.writeFileSync(path.join(home, ".claude/settings.json"), JSON.stringify(yours));

    const first = spawnSync("bash", [script, "claude"], { env, encoding: "utf8" });
    assert.equal(first.status, 0, first.stdout + first.stderr);
    const merged = JSON.parse(fs.readFileSync(path.join(home, ".claude/settings.json"), "utf8"));
    assert.equal(merged.model, "sonnet");
    assert.deepEqual(merged.statusLine, yours.statusLine, "a statusline of your own stays");
    assert.deepEqual(merged.permissions.allow, ["Bash(make *)", "Bash(git log *)", ...product.permissions.allow.filter((p) => p !== "Bash(git log *)")], "yours first, each rule once");
    assert.deepEqual(merged.permissions.deny, product.permissions.deny);
    assert.deepEqual(merged.hooks.Stop, [{ hooks: [{ type: "command", command: "say done" }] }, ...product.hooks.Stop], "your hook kept, Switchboard's once");
    assert.deepEqual(merged.hooks.PreToolUse, product.hooks.PreToolUse);
    assert.equal(fs.readdirSync(path.join(home, ".claude")).filter((f) => f.startsWith("settings.json.bak.")).length, 1, "a copy of yours left beside it");

    const again = spawnSync("bash", [script, "--check", "claude"], { env, encoding: "utf8" });
    assert.equal(again.stdout, "claude: up to date\n", "nothing left to merge");

    // A settings file linked into the repo, as the whole setup made it, becomes yours.
    fs.rmSync(path.join(home, ".claude/settings.json"));
    fs.symlinkSync(path.join(home, "switchboard/claude/settings.json"), path.join(home, ".claude/settings.json"));
    const linked = spawnSync("bash", [script, "claude"], { env, encoding: "utf8" });
    assert.equal(linked.status, 0, linked.stdout + linked.stderr);
    assert.match(linked.stdout, /make .*settings\.json your own/);
    assert.equal(fs.lstatSync(path.join(home, ".claude/settings.json")).isSymbolicLink(), false);
    assert.equal(fs.readFileSync(path.join(repo, "claude/settings.json"), "utf8"), execFileSync("git", ["-C", repo, "show", "HEAD:claude/settings.json"], { encoding: "utf8" }), "the repo's file untouched");
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test("setup.sh terminal adds Option as Alt to a Ghostty config of your own once, and leaves one that sets it alone", () => {
  const home = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "setup-terminal-")));
  // Ghostty, outside tmux, so nothing reaches Terminal's or iTerm2's preferences.
  const env = { ...process.env, HOME: home, TERM_PROGRAM: "ghostty", TMUX: "", LC_TERMINAL: "" };
  try {
    fs.symlinkSync(repo, path.join(home, "switchboard"));
    fs.mkdirSync(path.join(home, ".config/ghostty"), { recursive: true });
    const config = path.join(home, ".config/ghostty/config");
    fs.writeFileSync(config, "theme = mine\n");
    const first = spawnSync("bash", [script, "terminal"], { env, encoding: "utf8" });
    assert.equal(first.status, 0, first.stderr);
    assert.match(first.stdout, /add Switchboard's key settings to your Ghostty config/);
    assert.match(fs.readFileSync(config, "utf8"), /^theme = mine\n[\s\S]*\nconfig-file = \?~\/switchboard\/ghostty\/required\n$/);
    assert.match(fs.readFileSync(path.join(repo, "ghostty/required"), "utf8"), /^macos-option-as-alt = true$/m);
    const again = spawnSync("bash", [script, "terminal"], { env, encoding: "utf8" });
    assert.match(again.stdout, /terminal: up to date/, "a second run changes nothing");

    fs.writeFileSync(config, "macos-option-as-alt = left\n");
    const own = spawnSync("bash", [script, "terminal"], { env, encoding: "utf8" });
    assert.match(own.stdout, /terminal: up to date/, "a config that sets it itself is left alone");
    assert.equal(fs.readFileSync(config, "utf8"), "macos-option-as-alt = left\n");
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test("setup.sh claude writes your CLAUDE.md to import your rules, from a file or a repository, and Switchboard's part, once", async () => {
  const home = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "setup-claude-md-")));
  const env = { ...process.env, HOME: home, GIT_CONFIG_GLOBAL: "/dev/null", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e" };
  const file = path.join(home, ".claude/CLAUDE.md");
  const config = path.join(home, ".config/switchboard/claude.json");
  const run = () => spawnSync("bash", [script, "claude"], { env, encoding: "utf8" });
  try {
    fs.symlinkSync(repo, path.join(home, "switchboard"));
    fs.mkdirSync(path.join(home, ".claude"), { recursive: true });
    fs.mkdirSync(path.dirname(config), { recursive: true });

    // A CLAUDE.md of your own, no rules named: only the import is added, once.
    fs.writeFileSync(file, "# Mine\n\n- be brief\n");
    assert.equal(run().status, 0);
    assert.equal(fs.readFileSync(file, "utf8"), "# Mine\n\n- be brief\n\n@~/switchboard/claude/switchboard.md\n");
    run();
    assert.equal(fs.readFileSync(file, "utf8").match(/switchboard\.md/g).length, 1, "a second run adds nothing");

    // Rules in a file named in claude.json: your old CLAUDE.md is moved aside, and the new one imports both.
    const rules = path.join(home, "my-rules.md");
    fs.writeFileSync(rules, "# My rules\n");
    fs.writeFileSync(config, JSON.stringify({ path: "~/my-rules.md" }));
    const named = run();
    assert.match(named.stdout, /move your ~\/\.claude\/CLAUDE\.md aside/);
    assert.match(fs.readFileSync(file, "utf8"), /^<!-- Written by Switchboard's setup\.sh[^\n]*-->\n\n@~\/my-rules\.md\n@~\/switchboard\/claude\/switchboard\.md\n$/);
    assert.ok(fs.readdirSync(path.join(home, ".claude")).some((n) => n.startsWith("CLAUDE.md.bak.")));
    assert.match(run().stdout, /claude: up to date/, "a second run changes nothing");

    // Rules in a repository: cloned, and its file imported.
    const origin = path.join(home, "rules.git");
    const work = path.join(home, "rules-work");
    execFileSync("git", ["init", "-q", "-b", "main", work], { env });
    fs.writeFileSync(path.join(work, "CLAUDE.md"), "# From my repo\n");
    execFileSync("git", ["-C", work, "add", "."], { env });
    execFileSync("git", ["-C", work, "commit", "-q", "-m", "rules"], { env });
    execFileSync("git", ["clone", "-q", "--bare", work, origin], { env });
    fs.writeFileSync(config, JSON.stringify({ repo: origin }));
    assert.equal(run().status, 0);
    assert.match(fs.readFileSync(file, "utf8"), /\n@~\/\.local\/share\/switchboard\/claude-md\/CLAUDE\.md\n@~\/switchboard\/claude\/switchboard\.md\n$/);
    assert.equal(fs.readFileSync(path.join(home, ".local/share/switchboard/claude-md/CLAUDE.md"), "utf8"), "# From my repo\n");
    assert.ok(!fs.readdirSync(path.join(home, ".claude")).filter((n) => n.startsWith("CLAUDE.md.bak.")).slice(1).length, "its own written file isn't moved aside again");

    // One run at a time: a run finding the lock held waits for it, and a lock
    // its holder left, by ending, is free.
    const lock = path.join(home, ".local/state/switchboard/claude-md.lock");
    fs.mkdirSync(path.dirname(lock), { recursive: true });
    const holder = require("node:child_process").spawn("lockf", ["-k", lock, "sleep", "3"]);
    execFileSync("sleep", ["0.5"]);
    const started = Date.now();
    assert.equal(spawnSync("bash", [script, "claude_md"], { env, encoding: "utf8" }).status, 0);
    assert.ok(Date.now() - started > 1500, "waited for the holder");
    await new Promise((resolve) => holder.on("exit", resolve));

    // The update check's run only pulls: a repository not cloned yet is left.
    fs.rmSync(path.join(home, ".local/share/switchboard/claude-md"), { recursive: true, force: true });
    assert.equal(spawnSync("bash", [script, "claude_md"], { env: { ...env, SWITCHBOARD_PULL_ONLY: "true" }, encoding: "utf8" }).status, 0);
    assert.ok(!fs.existsSync(path.join(home, ".local/share/switchboard/claude-md")));
    assert.equal(run().status, 0);

    // Another repository, through the claude_md part alone, as mission
    // control's Settings runs it: the old clone is moved aside, the new one cloned.
    fs.writeFileSync(path.join(work, "CLAUDE.md"), "# From my other repo\n");
    execFileSync("git", ["-C", work, "commit", "-q", "-am", "other"], { env });
    const other = path.join(home, "other.git");
    execFileSync("git", ["clone", "-q", "--bare", work, other], { env });
    fs.writeFileSync(config, JSON.stringify({ repo: other }));
    assert.equal(spawnSync("bash", [script, "claude_md"], { env, encoding: "utf8" }).status, 0);
    assert.equal(fs.readFileSync(path.join(home, ".local/share/switchboard/claude-md/CLAUDE.md"), "utf8"), "# From my other repo\n");
    assert.ok(fs.readdirSync(path.join(home, ".local/share/switchboard")).some((n) => n.startsWith("claude-md.bak.")));

    // A repository keeping it in a folder: the one nested least deep.
    const clone = path.join(home, ".local/share/switchboard/claude-md");
    execFileSync("git", ["-C", work, "rm", "-q", "CLAUDE.md"], { env });
    fs.mkdirSync(path.join(work, "claude/deeper"), { recursive: true });
    fs.writeFileSync(path.join(work, "claude/CLAUDE.md"), "# Nested\n");
    fs.writeFileSync(path.join(work, "claude/deeper/CLAUDE.md"), "# Deeper\n");
    execFileSync("git", ["-C", work, "add", "."], { env });
    execFileSync("git", ["-C", work, "commit", "-q", "-m", "nested"], { env });
    execFileSync("git", ["-C", work, "push", "-q", other, "main"], { env });
    assert.equal(spawnSync("bash", [script, "claude_md"], { env, encoding: "utf8" }).status, 0);
    assert.match(fs.readFileSync(file, "utf8"), /\n@~\/\.local\/share\/switchboard\/claude-md\/claude\/CLAUDE\.md\n/);

    // Two at that depth: which one is yours, asked for, and the file left as it was.
    const before = fs.readFileSync(file, "utf8");
    fs.mkdirSync(path.join(work, "other"));
    fs.writeFileSync(path.join(work, "other/CLAUDE.md"), "# Another\n");
    execFileSync("git", ["-C", work, "add", "."], { env });
    execFileSync("git", ["-C", work, "commit", "-q", "-m", "two"], { env });
    execFileSync("git", ["-C", work, "push", "-q", other, "main"], { env });
    const two = spawnSync("bash", [script, "claude_md"], { env, encoding: "utf8" });
    assert.notEqual(two.status, 0);
    assert.match(two.stderr, /more than one CLAUDE\.md \(claude\/CLAUDE\.md, other\/CLAUDE\.md\): name yours with "file"/);
    assert.equal(fs.readFileSync(file, "utf8"), before);
    fs.writeFileSync(config, JSON.stringify({ repo: other, file: "other/CLAUDE.md" }));
    assert.equal(spawnSync("bash", [script, "claude_md"], { env, encoding: "utf8" }).status, 0);
    assert.match(fs.readFileSync(file, "utf8"), /claude-md\/other\/CLAUDE\.md\n/);

    // None at all, or a file named that isn't there: said, not imported as nothing.
    fs.writeFileSync(config, JSON.stringify({ repo: other, file: "gone.md" }));
    assert.match(spawnSync("bash", [script, "claude_md"], { env, encoding: "utf8" }).stderr, /your rules aren't at ~\/\.local\/share\/switchboard\/claude-md\/gone\.md/);
    execFileSync("git", ["-C", work, "rm", "-q", "-r", "claude", "other"], { env });

    // A folder named with accents and quotes, which git would print escaped.
    const odd = 'résumé "notes"';
    fs.mkdirSync(path.join(work, odd));
    fs.writeFileSync(path.join(work, odd, "CLAUDE.md"), "# Odd\n");
    execFileSync("git", ["-C", work, "add", "."], { env });
    execFileSync("git", ["-C", work, "commit", "-q", "-m", "odd"], { env });
    execFileSync("git", ["-C", work, "push", "-q", other, "main"], { env });
    fs.writeFileSync(config, JSON.stringify({ repo: other }));
    assert.equal(spawnSync("bash", [script, "claude_md"], { env, encoding: "utf8" }).status, 0);
    assert.ok(fs.readFileSync(file, "utf8").includes(`claude-md/${odd}/CLAUDE.md\n`));

    execFileSync("git", ["-C", work, "rm", "-q", "-r", odd], { env });
    fs.writeFileSync(path.join(work, "README.md"), "no rules\n");
    // A folder named CLAUDE.md holds no rules.
    fs.mkdirSync(path.join(work, "CLAUDE.md"));
    fs.writeFileSync(path.join(work, "CLAUDE.md/README.md"), "not rules\n");
    execFileSync("git", ["-C", work, "add", "."], { env });
    execFileSync("git", ["-C", work, "commit", "-q", "-m", "none"], { env });
    execFileSync("git", ["-C", work, "push", "-q", other, "main"], { env });
    fs.writeFileSync(config, JSON.stringify({ repo: other }));
    const none = spawnSync("bash", [script, "claude_md"], { env, encoding: "utf8" });
    assert.notEqual(none.status, 0);
    assert.match(none.stderr, /no CLAUDE\.md in your rules' repository/);
    assert.ok(fs.existsSync(clone));

    // The guided setup lists it as left to do, with Claude Code there.
    const bin = path.join(home, "bin");
    fs.mkdirSync(bin);
    fs.writeFileSync(path.join(bin, "claude"), "#!/bin/sh\n", { mode: 0o755 });
    const guided = spawnSync("bash", [script, "--check"], { env: { ...env, PATH: `${bin}:${env.PATH}` }, encoding: "utf8" });
    assert.match(guided.stdout, /✗ your CLAUDE\.md: no CLAUDE\.md in your rules' repository/);
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test("setup.sh claude_md adds gh as a credential helper of a rules clone on GitHub, once", () => {
  const home = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "setup-claude-md-gh-")));
  // A space in gh's path, which the helper has to quote.
  const bin = path.join(home, "my bin");
  // No network: the clone's update goes to a proxy that isn't there, and fails at once.
  const env = { ...process.env, HOME: home, PATH: `${bin}:${process.env.PATH}`, GIT_CONFIG_GLOBAL: "/dev/null", GIT_TERMINAL_PROMPT: "0", GIT_CONFIG_COUNT: "1", GIT_CONFIG_KEY_0: "http.proxy", GIT_CONFIG_VALUE_0: "http://127.0.0.1:9", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e" };
  const clone = path.join(home, ".local/share/switchboard/claude-md");
  const url = "https://gist.github.com/abc123.git";
  try {
    fs.symlinkSync(repo, path.join(home, "switchboard"));
    fs.mkdirSync(bin);
    // gh signed in only once SIGNED_IN is there.
    fs.writeFileSync(path.join(bin, "gh"), `#!/bin/sh\n[ "$1 $2" = "auth token" ] && [ -e "${home}/SIGNED_IN" ]\n`, { mode: 0o755 });
    fs.mkdirSync(path.join(home, ".config/switchboard"), { recursive: true });
    fs.writeFileSync(path.join(home, ".config/switchboard/claude.json"), JSON.stringify({ repo: url }));
    execFileSync("git", ["init", "-q", "-b", "main", clone], { env });
    fs.writeFileSync(path.join(clone, "CLAUDE.md"), "# Rules\n");
    execFileSync("git", ["-C", clone, "add", "."], { env });
    execFileSync("git", ["-C", clone, "commit", "-q", "-m", "rules"], { env });
    execFileSync("git", ["-C", clone, "remote", "add", "origin", url], { env });
    execFileSync("git", ["-C", clone, "config", "credential.helper", "osxkeychain"], { env });

    // Not signed in to gh: the clone's own helper is kept.
    spawnSync("bash", [script, "claude_md"], { env, encoding: "utf8" });
    assert.equal(execFileSync("git", ["-C", clone, "config", "--local", "--get-all", "credential.helper"], { env, encoding: "utf8" }), "osxkeychain\n");

    fs.writeFileSync(path.join(home, "SIGNED_IN"), "");
    assert.match(spawnSync("bash", [script, "--check", "claude_md"], { env, encoding: "utf8" }).stdout, /add gh as a credential helper for your CLAUDE\.md's repository/);
    assert.equal(spawnSync("bash", [script, "claude_md"], { env, encoding: "utf8" }).status, 0);
    const helpers = execFileSync("git", ["-C", clone, "config", "--local", "--get-all", "credential.helper"], { env, encoding: "utf8" });
    assert.equal(helpers, `osxkeychain\n!'${path.join(bin, "gh")}' auth git-credential\n`, "gh's, quoted, after the clone's own, which git asks first");
    assert.doesNotMatch(spawnSync("bash", [script, "claude_md"], { env, encoding: "utf8" }).stdout, /credential helper/, "a second run changes nothing");
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test("setup.sh skills links Switchboard's skills, keeping one of your own by the same name, once", () => {
  const home = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "setup-skills-")));
  const env = { ...process.env, HOME: home };
  const skills = path.join(home, ".claude/skills");
  const shipped = fs.readdirSync(path.join(repo, "claude/skills"));
  try {
    fs.symlinkSync(repo, path.join(home, "switchboard"));
    // Your own wt, and a shape linked from a clone by its old name.
    fs.mkdirSync(path.join(skills, "wt"), { recursive: true });
    fs.writeFileSync(path.join(skills, "wt/SKILL.md"), "mine\n");
    fs.symlinkSync("/somewhere/editor-configs/claude/skills/shape", path.join(skills, "shape"));
    // A Switchboard link made by hand, ending in a /: Switchboard's, so written again as setup writes it.
    fs.symlinkSync(path.join(home, "switchboard/claude/skills/oncall/"), path.join(skills, "oncall"));
    // A note-watch of your own, linked from dotfiles laid out the same way.
    fs.mkdirSync(path.join(home, "dotfiles/claude/skills/note-watch"), { recursive: true });
    fs.symlinkSync(path.join(home, "dotfiles/claude/skills/note-watch"), path.join(skills, "note-watch"));
    // And a today linked relatively, read from the folder the link is in.
    fs.mkdirSync(path.join(home, "dotfiles/claude/skills/today"), { recursive: true });
    fs.symlinkSync("../../dotfiles/claude/skills/today", path.join(skills, "today"));

    assert.match(spawnSync("bash", [script, "--check", "skills"], { env, encoding: "utf8" }).stdout, /link .*\/\.claude\/skills\/review-loop/);
    assert.ok(!fs.existsSync(path.join(skills, "review-loop")), "--check changes nothing");
    assert.equal(spawnSync("bash", [script, "skills"], { env, encoding: "utf8" }).status, 0);
    shipped.filter((name) => !["wt", "note-watch", "today"].includes(name)).forEach((name) => assert.equal(fs.readlinkSync(path.join(skills, name)), path.join(home, "switchboard/claude/skills", name)));
    assert.equal(fs.readlinkSync(path.join(home, ".local/bin/skill-path")), path.join(home, "switchboard/bin/skill-path"), "with skill-path, which every skill runs first");
    assert.equal(fs.readFileSync(path.join(skills, "wt/SKILL.md"), "utf8"), "mine\n", "your own is kept");
    assert.equal(fs.readlinkSync(path.join(skills, "note-watch")), path.join(home, "dotfiles/claude/skills/note-watch"), "a link of your own too");
    assert.equal(fs.readlinkSync(path.join(skills, "today")), "../../dotfiles/claude/skills/today", "a relative one as well");
    assert.ok(!fs.readdirSync(skills).some((name) => name.includes(".bak.")), "and nothing moved aside");
    assert.match(spawnSync("bash", [script, "skills"], { env, encoding: "utf8" }).stdout, /skills: up to date/, "a second run changes nothing");

    // One that can't be linked doesn't stop the rest, and is said at the end.
    fs.rmSync(path.join(skills, "ci-loop"));
    fs.rmSync(path.join(skills, "wt"), { recursive: true });
    fs.rmSync(path.join(skills, "shape"));
    fs.mkdirSync(path.join(skills, "ci-loop"));
    fs.chmodSync(skills, 0o555);
    const failed = spawnSync("bash", [script, "skills"], { env, encoding: "utf8" });
    fs.chmodSync(skills, 0o755);
    assert.notEqual(failed.status, 0);
    assert.match(failed.stderr, /could not link these skills into ~\/\.claude\/skills: shape wt$/m, "every one tried, the failures said together");
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test("setup.sh --parts lists every part with what it does, starting no interactive zsh", () => {
  // tmux's PATH has no node, which has setup.sh ask an interactive zsh for
  // it; M-p pipes --parts into fzf, and that zsh took the popup's terminal
  // from it. A stand-in zsh records being started.
  const bin = fs.mkdtempSync(path.join(os.tmpdir(), "setup-parts-"));
  const started = path.join(bin, "started");
  fs.writeFileSync(path.join(bin, "zsh"), `#!/bin/sh\necho "$@" >> '${started}'\n`, { mode: 0o755 });
  const listed = spawnSync("bash", [script, "--parts"], { encoding: "utf8", env: { ...process.env, PATH: `${bin}:/usr/bin:/bin` } });
  assert.equal(fs.existsSync(started), false, "no zsh started");
  fs.rmSync(bin, { recursive: true, force: true });
  assert.equal(listed.status, 0);
  const rows = listed.stdout.trim().split("\n").map((line) => line.split("\t"));
  const parts = fs.readFileSync(script, "utf8").match(/^  PARTS=\((.*)\)$/m)[1].split(" ");
  assert.deepEqual(rows.map(([name]) => name), parts);
  rows.forEach(([name, about]) => assert.ok(about && about !== name, `${name} says what it does`));
});

test("setup.sh config (and tmux) moves the AWS login's Chrome profile into browser.json, once", () => {
  const home = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "setup-aws-")));
  const env = { ...process.env, HOME: home };
  try {
    fs.symlinkSync(repo, path.join(home, "switchboard"));
    fs.mkdirSync(path.join(home, ".config/aws-login"), { recursive: true });
    fs.mkdirSync(path.join(home, ".config/switchboard"), { recursive: true });
    fs.writeFileSync(path.join(home, ".config/aws-login/config"), "# mine\nchrome_profile=Profile 3\n");
    fs.writeFileSync(path.join(home, ".config/switchboard/browser.json"), JSON.stringify({ hosts: { "slack.com": "Profile 2" } }));
    const run = () => spawnSync("bash", [script, "--check", "config"], { env, encoding: "utf8" });
    assert.match(run().stdout, /move the AWS login's Chrome profile \(Profile 3\)/);
    assert.ok(fs.existsSync(path.join(home, ".config/aws-login/config")), "--check changes nothing");
    const result = spawnSync("bash", [script, "config"], { env, encoding: "utf8" });
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(JSON.parse(fs.readFileSync(path.join(home, ".config/switchboard/browser.json"), "utf8")), { hosts: { "slack.com": "Profile 2", "awsapps.com": "Profile 3" } });
    assert.ok(!fs.existsSync(path.join(home, ".config/aws-login/config")));
    assert.ok(fs.readdirSync(path.join(home, ".config/aws-login")).some((n) => n.startsWith("config.bak.")));
    assert.doesNotMatch(run().stdout, /AWS login/, "a second run has nothing to move");
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test("setup.sh config moves each tool's own config into ~/.config/switchboard, keeping one already there", () => {
  const home = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "setup-configs-")));
  const env = { ...process.env, HOME: home };
  try {
    fs.symlinkSync(repo, path.join(home, "switchboard"));
    fs.mkdirSync(path.join(home, ".config/worktree-from-ticket"), { recursive: true });
    fs.mkdirSync(path.join(home, ".config/oncall"), { recursive: true });
    fs.mkdirSync(path.join(home, ".config/tmux-sidebar/plugins"), { recursive: true });
    fs.mkdirSync(path.join(home, ".config/switchboard"), { recursive: true });
    fs.writeFileSync(path.join(home, ".config/worktree-from-ticket/config.json"), JSON.stringify({ linearTeam: "ABC" }));
    fs.writeFileSync(path.join(home, ".config/oncall/config.json"), JSON.stringify({ channels: [], rules: "~/.config/oncall/rules.md" }));
    fs.writeFileSync(path.join(home, ".config/oncall/rules.md"), "# Rules\n");
    fs.writeFileSync(path.join(home, ".config/tmux-sidebar/plugins/demo.js"), "module.exports = {};\n");
    fs.mkdirSync(path.join(home, "outside"), { recursive: true });
    fs.writeFileSync(path.join(home, "outside/beside.js"), "module.exports = {};\n");
    fs.symlinkSync("../../../outside/beside.js", path.join(home, ".config/tmux-sidebar/plugins/beside.js"));
    fs.mkdirSync(path.join(home, ".config/tmux-sidebar/plugins/shared"));
    fs.writeFileSync(path.join(home, ".config/tmux-sidebar/plugins/shared/impl.js"), "module.exports = {};\n");
    fs.symlinkSync("shared/impl.js", path.join(home, ".config/tmux-sidebar/plugins/inner.js"));
    fs.symlinkSync("../plugins/shared/impl.js", path.join(home, ".config/tmux-sidebar/plugins/named.js"));
    fs.symlinkSync(".", path.join(home, ".config/tmux-sidebar/plugins/self"));
    fs.symlinkSync(path.join(home, ".config/tmux-sidebar/plugins/shared/impl.js"), path.join(home, ".config/tmux-sidebar/plugins/absolute.js"));
    fs.symlinkSync(path.join(home, ".config/tmux-sidebar/plugins/absolute.js"), path.join(home, ".config/tmux-sidebar/plugins/chained.js"));
    // One already moved by hand: kept, and the old one left.
    fs.writeFileSync(path.join(home, ".config/tmux-sidebar/config.json"), JSON.stringify({ plugins: ["old"] }));
    fs.writeFileSync(path.join(home, ".config/switchboard/sidebar.json"), JSON.stringify({ plugins: ["demo"] }));
    const result = spawnSync("bash", [script, "config"], { env, encoding: "utf8" });
    assert.equal(result.status, 0, result.stderr);
    const read = (p) => fs.readFileSync(path.join(home, ".config/switchboard", p), "utf8");
    assert.deepEqual(JSON.parse(read("linear.json")), { linearTeam: "ABC" });
    assert.deepEqual(JSON.parse(read("oncall.json")), { channels: [], rules: "~/.config/switchboard/oncall-rules.md" });
    assert.equal(read("oncall-rules.md"), "# Rules\n");
    assert.ok(fs.existsSync(path.join(home, ".config/switchboard/sidebar-plugins/demo.js")));
    assert.ok(fs.existsSync(path.join(home, ".config/switchboard/sidebar-plugins/beside.js")), "a relative link in the folder still reaches its file");
    assert.equal(fs.readlinkSync(path.join(home, ".config/switchboard/sidebar-plugins/inner.js")), path.join(home, ".config/switchboard/sidebar-plugins/shared/impl.js"), "one to a file in the folder follows it");
    assert.ok(fs.existsSync(path.join(home, ".config/switchboard/sidebar-plugins/named.js")), "one that names the folder too");
    assert.ok(fs.existsSync(path.join(home, ".config/switchboard/sidebar-plugins/self/demo.js")), "and one to the folder itself");
    assert.ok(fs.existsSync(path.join(home, ".config/switchboard/sidebar-plugins/inner.js")));
    assert.ok(fs.existsSync(path.join(home, ".config/switchboard/sidebar-plugins/absolute.js")), "an absolute one into the folder follows its file");
    assert.ok(fs.existsSync(path.join(home, ".config/switchboard/sidebar-plugins/chained.js")), "and one leading through another");
    assert.deepEqual(JSON.parse(read("sidebar.json")), { plugins: ["demo"] });
    assert.ok(fs.existsSync(path.join(home, ".config/tmux-sidebar/config.json")));
    assert.match(result.stderr, /tmux-sidebar\/config.json is still there/);
    assert.ok(!fs.existsSync(path.join(home, ".config/worktree-from-ticket")), "its emptied directory goes");
    assert.doesNotMatch(spawnSync("bash", [script, "config"], { env, encoding: "utf8" }).stdout, /move/, "a second run moves nothing");

    // Plugins with something else beside their folder, which they may load,
    // stay where they are, with a word about it.
    fs.mkdirSync(path.join(home, ".config/tmux-sidebar/plugins"), { recursive: true });
    fs.mkdirSync(path.join(home, ".config/tmux-sidebar/node_modules"), { recursive: true });
    fs.renameSync(path.join(home, ".config/switchboard/sidebar-plugins"), path.join(home, "kept-plugins"));
    const beside = spawnSync("bash", [script, "config"], { env, encoding: "utf8" });
    assert.match(beside.stdout, /link ~\/.config\/switchboard\/sidebar-plugins to ~\/.config\/tmux-sidebar\/plugins/);
    assert.ok(fs.existsSync(path.join(home, ".config/tmux-sidebar/plugins")));
    assert.equal(fs.readlinkSync(path.join(home, ".config/switchboard/sidebar-plugins")), fs.realpathSync(path.join(home, ".config/tmux-sidebar/plugins")), "linked, so the plugins still load");
    fs.rmSync(path.join(home, ".config/switchboard/sidebar-plugins"));
    fs.rmSync(path.join(home, ".config/tmux-sidebar"), { recursive: true });

    // A link moves as a link to the same file; a rules path written in full is updated.
    fs.mkdirSync(path.join(home, "dots"), { recursive: true });
    fs.writeFileSync(path.join(home, "dots/relay.json"), JSON.stringify({ owner: "U1" }));
    fs.mkdirSync(path.join(home, ".config/slack-relay"), { recursive: true });
    fs.symlinkSync("../../dots/relay.json", path.join(home, ".config/slack-relay/config.json"));
    fs.rmSync(path.join(home, ".config/switchboard/oncall.json"));
    fs.rmSync(path.join(home, ".config/switchboard/oncall-rules.md"));
    fs.mkdirSync(path.join(home, ".config/oncall"), { recursive: true });
    fs.writeFileSync(path.join(home, ".config/oncall/config.json"), JSON.stringify({ rules: path.join(home, ".config/oncall/./rules.md") }));
    fs.writeFileSync(path.join(home, ".config/oncall/rules.md"), "# Rules\n");
    assert.equal(spawnSync("bash", [script, "config"], { env, encoding: "utf8" }).status, 0);
    assert.equal(fs.readlinkSync(path.join(home, ".config/switchboard/slack-relay.json")), path.join(home, "dots/relay.json"));
    assert.deepEqual(JSON.parse(read("oncall.json")), { rules: "~/.config/switchboard/oncall-rules.md" });

    // A linked on-call config keeps its link, the rules path changed in the file it points to.
    fs.rmSync(path.join(home, ".config/switchboard/oncall.json"));
    fs.rmSync(path.join(home, ".config/switchboard/oncall-rules.md"));
    fs.writeFileSync(path.join(home, "dots/oncall.json"), JSON.stringify({ rules: "~/.config/oncall/rules.md" }), { mode: 0o600 });
    fs.mkdirSync(path.join(home, ".config/oncall"), { recursive: true });
    fs.symlinkSync(path.join(home, "dots/oncall.json"), path.join(home, ".config/oncall/config.json"));
    fs.writeFileSync(path.join(home, ".config/oncall/rules.md"), "# Rules\n");
    assert.equal(spawnSync("bash", [script, "config"], { env, encoding: "utf8" }).status, 0);
    assert.ok(fs.lstatSync(path.join(home, ".config/switchboard/oncall.json")).isSymbolicLink());
    assert.deepEqual(JSON.parse(fs.readFileSync(path.join(home, "dots/oncall.json"), "utf8")), { rules: "~/.config/switchboard/oncall-rules.md" });

    assert.equal(fs.statSync(path.join(home, "dots/oncall.json")).mode & 0o777, 0o600, "its mode kept");

    // An on-call config already moved that names the old rules file: pointed
    // at the rules file's new place when that moves.
    fs.writeFileSync(path.join(home, ".config/switchboard/oncall.json"), JSON.stringify({ rules: "~/.config/oncall/rules.md" }));
    fs.rmSync(path.join(home, ".config/switchboard/oncall-rules.md"), { force: true });
    fs.mkdirSync(path.join(home, ".config/oncall"), { recursive: true });
    fs.writeFileSync(path.join(home, ".config/oncall/rules.md"), "# Rules\n");
    spawnSync("bash", [script, "config"], { env, encoding: "utf8" });
    assert.deepEqual(JSON.parse(read("oncall.json")), { rules: "~/.config/switchboard/oncall-rules.md" });
    assert.equal(read("oncall-rules.md"), "# Rules\n");

    // One old config linked to another that moves first: linked to its new place.
    fs.rmSync(path.join(home, ".config/switchboard/slack-relay.json"), { force: true });
    fs.rmSync(path.join(home, ".config/switchboard/slack-updates.json"), { force: true });
    fs.mkdirSync(path.join(home, ".config/slack-relay"), { recursive: true });
    fs.mkdirSync(path.join(home, ".config/slack-updates"), { recursive: true });
    fs.writeFileSync(path.join(home, ".config/slack-relay/config.json"), JSON.stringify({ owner: "U1", channels: ["C1"] }));
    fs.symlinkSync("../slack-relay/config.json", path.join(home, ".config/slack-updates/config.json"));
    spawnSync("bash", [script, "config"], { env, encoding: "utf8" });
    assert.equal(fs.readlinkSync(path.join(home, ".config/switchboard/slack-updates.json")), path.join(home, ".config/switchboard/slack-relay.json"));
    assert.deepEqual(JSON.parse(read("slack-updates.json")), { owner: "U1", channels: ["C1"] });
    fs.rmSync(path.join(home, ".config/switchboard/slack-updates.json"));

    // Linked to one that stays (its new place already taken): still linked to it.
    fs.mkdirSync(path.join(home, ".config/slack-relay"), { recursive: true });
    fs.mkdirSync(path.join(home, ".config/slack-updates"), { recursive: true });
    fs.writeFileSync(path.join(home, ".config/slack-relay/config.json"), JSON.stringify({ channels: ["C2"] }));
    fs.symlinkSync("../slack-relay/config.json", path.join(home, ".config/slack-updates/config.json"));
    spawnSync("bash", [script, "config"], { env, encoding: "utf8" });
    assert.deepEqual(JSON.parse(read("slack-updates.json")), { channels: ["C2"] });
    assert.deepEqual(JSON.parse(read("slack-relay.json")), { owner: "U1", channels: ["C1"] });
    fs.rmSync(path.join(home, ".config/switchboard/slack-updates.json"));
    fs.rmSync(path.join(home, ".config/slack-relay"), { recursive: true });

    // A link to nothing already there is kept, the old file left beside it.
    fs.mkdirSync(path.join(home, ".config/slack-updates"), { recursive: true });
    fs.writeFileSync(path.join(home, ".config/slack-updates/config.json"), "{}");
    fs.symlinkSync(path.join(home, "gone.json"), path.join(home, ".config/switchboard/slack-updates.json"));
    spawnSync("bash", [script, "config"], { env, encoding: "utf8" });
    assert.equal(fs.readlinkSync(path.join(home, ".config/switchboard/slack-updates.json")), path.join(home, "gone.json"));
    assert.ok(fs.existsSync(path.join(home, ".config/slack-updates/config.json")));
    fs.rmSync(path.join(home, ".config/switchboard/slack-updates.json"));
    fs.rmSync(path.join(home, ".config/slack-updates"), { recursive: true });

    // The Linear config under XDG_CONFIG_HOME, where worktree-from-ticket read it,
    // over one left in ~/.config.
    fs.rmSync(path.join(home, ".config/switchboard/linear.json"));
    fs.mkdirSync(path.join(home, ".config/worktree-from-ticket"), { recursive: true });
    fs.writeFileSync(path.join(home, ".config/worktree-from-ticket/config.json"), JSON.stringify({ linearTeam: "STALE" }));
    fs.mkdirSync(path.join(home, "xdg/worktree-from-ticket"), { recursive: true });
    fs.writeFileSync(path.join(home, "xdg/worktree-from-ticket/config.json"), JSON.stringify({ linearTeam: "XDG" }));
    assert.equal(spawnSync("bash", [script, "config"], { env: { ...env, XDG_CONFIG_HOME: path.join(home, "xdg") }, encoding: "utf8" }).status, 0);
    assert.deepEqual(JSON.parse(read("linear.json")), { linearTeam: "XDG" });

    // A linked browser.json keeps its link, the profile added in the file it points to.
    fs.writeFileSync(path.join(home, "dots/browser.json"), JSON.stringify({ hosts: { "a.com": "P" } }));
    fs.rmSync(path.join(home, ".config/switchboard/browser.json"), { force: true });
    fs.symlinkSync(path.join(home, "dots/browser.json"), path.join(home, ".config/switchboard/browser.json"));
    fs.mkdirSync(path.join(home, ".config/aws-login"), { recursive: true });
    fs.writeFileSync(path.join(home, ".config/aws-login/config"), "chrome_profile=Profile 3\n");
    assert.equal(spawnSync("bash", [script, "config"], { env, encoding: "utf8" }).status, 0);
    assert.ok(fs.lstatSync(path.join(home, ".config/switchboard/browser.json")).isSymbolicLink());
    assert.deepEqual(JSON.parse(fs.readFileSync(path.join(home, "dots/browser.json"), "utf8")), { hosts: { "a.com": "P", "awsapps.com": "Profile 3" } });
    fs.rmSync(path.join(home, ".config/switchboard/browser.json"));

    // A browser.json that can't be read is never written over.
    fs.mkdirSync(path.join(home, ".config/aws-login"), { recursive: true });
    fs.writeFileSync(path.join(home, ".config/aws-login/config"), "chrome_profile=Profile 3\n");
    fs.writeFileSync(path.join(home, ".config/switchboard/browser.json"), '{"hosts": {"a.com": "P"},}');
    fs.mkdirSync(path.join(home, ".config/slack-updates"), { recursive: true });
    fs.writeFileSync(path.join(home, ".config/slack-updates/config.json"), JSON.stringify({ channels: ["C1"] }));
    const broken = spawnSync("bash", [script, "config"], { env, encoding: "utf8" });
    assert.match(broken.stderr, /browser.json can't be read/);
    assert.deepEqual(JSON.parse(read("slack-updates.json")), { channels: ["C1"] }, "the other files still move");
    assert.equal(read("browser.json"), '{"hosts": {"a.com": "P"},}');
    assert.ok(fs.existsSync(path.join(home, ".config/aws-login/config")));
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test("setup.sh claude turns a CLAUDE.md linked into the repo's old claude/CLAUDE.md into your own rules file, from git once it is gone", () => {
  const home = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "setup-claude-link-")));
  const env = { ...process.env, HOME: home };
  try {
    fs.symlinkSync(repo, path.join(home, "switchboard"));
    fs.mkdirSync(path.join(home, ".claude"), { recursive: true });
    fs.symlinkSync(path.join(repo, "claude/CLAUDE.md"), path.join(home, ".claude/CLAUDE.md"));
    fs.mkdirSync(path.join(home, ".config/switchboard"), { recursive: true });
    const result = spawnSync("bash", [script, "claude"], { env, encoding: "utf8" });
    assert.equal(result.status, 0, result.stderr);
    const rules = fs.readFileSync(path.join(home, ".config/switchboard/CLAUDE.md"), "utf8");
    assert.match(rules, /^# /, "the rules it linked to, from git history");
    assert.doesNotMatch(rules, /^@~\/switchboard\/claude\/switchboard\.md$/m);
    assert.ok(!fs.lstatSync(path.join(home, ".claude/CLAUDE.md")).isSymbolicLink());
    assert.equal(fs.readlinkSync(path.join(home, ".local/bin/skill-path")), path.join(home, "switchboard/bin/skill-path"), "skill-path comes with the skills, which run it first");
    assert.match(fs.readFileSync(path.join(home, ".claude/CLAUDE.md"), "utf8"), /\n@~\/\.config\/switchboard\/CLAUDE\.md\n@~\/switchboard\/claude\/switchboard\.md\n$/);
    assert.match(spawnSync("bash", [script, "claude"], { env, encoding: "utf8" }).stdout, /claude: up to date/, "a second run changes nothing");
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test("the post-commit hook runs setup.sh tmux bin in ~/switchboard after a commit adds a file under tmux/ or bin/", () => {
  const home = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "setup-commit-")));
  const clone = path.join(home, "switchboard");
  const env = { ...process.env, HOME: home, GIT_CONFIG_GLOBAL: "/dev/null", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@t", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@t" };
  delete env.GIT_REFLOG_ACTION;
  const git = (...args) => execFileSync("git", ["-C", clone, ...args], { env, encoding: "utf8" });
  const ran = path.join(home, "ran");
  const commit = (file) => {
    fs.mkdirSync(path.dirname(path.join(clone, file)), { recursive: true });
    fs.writeFileSync(path.join(clone, file), String(Math.random()));
    git("add", file);
    return git("commit", "-qm", file);
  };
  try {
    fs.mkdirSync(path.join(clone, ".githooks"), { recursive: true });
    git("init", "-q");
    fs.copyFileSync(path.join(repo, ".githooks/post-commit"), path.join(clone, ".githooks/post-commit"));
    fs.chmodSync(path.join(clone, ".githooks/post-commit"), 0o755);
    // A stand-in for setup.sh that says what it was asked to do.
    fs.writeFileSync(path.join(clone, "setup.sh"), `#!/bin/sh\necho "$*" >> ${JSON.stringify(ran)}\necho "tmux:\n  link new.sh"\n`, { mode: 0o755 });
    git("config", "core.hooksPath", ".githooks");

    commit("README.md");
    assert.equal(fs.existsSync(ran), false, "nothing under tmux/ or bin/ added");
    commit("tmux/new.sh");
    assert.equal(fs.readFileSync(ran, "utf8"), "tmux bin\n");
    commit("tmux/new.sh");
    assert.equal(fs.readFileSync(ran, "utf8"), "tmux bin\n", "a change to a file there adds nothing to link");
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test("setup.sh's tmux part restarts a sidebar daemon older than ~/switchboard's sidebar code, once, and only says so with --check", async () => {
  const home = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "setup-daemon-")));
  const env = { ...process.env, HOME: home };
  const state = path.join(home, ".local/state/tmux-sidebar");
  const started = path.join(home, "started");
  // restart_stale_daemon alone, with the change reporting it uses, since the
  // whole tmux part fetches the plugins.
  const source = fs.readFileSync(script, "utf8");
  const fn = source.match(/^restart_stale_daemon\(\) \{\n[\s\S]*?\n\}\n/m)[0];
  const run = (check) =>
    spawnSync("bash", ["-c", `SRC=$HOME/switchboard CHECK=${check} changes=0\nchange() { echo "$1"; changes=$((changes + 1)); }\n${fn}restart_stale_daemon; echo "changes=$changes"`], { env, encoding: "utf8" });
  let daemon;
  try {
    fs.mkdirSync(path.join(home, "switchboard/lib/sidebar"), { recursive: true });
    fs.mkdirSync(path.join(home, "switchboard/bin"), { recursive: true });
    fs.writeFileSync(path.join(home, "switchboard/lib/sidebar/model.js"), "");
    fs.writeFileSync(path.join(home, "switchboard/bin/tmux-sidebar"), "setInterval(() => {}, 60_000);\n");
    daemon = spawn(process.execPath, [path.join(home, "switchboard/bin/tmux-sidebar"), "daemon"], { stdio: "ignore" });
    fs.mkdirSync(state, { recursive: true });
    fs.writeFileSync(path.join(state, "daemon.pid"), String(daemon.pid));
    const old = new Date(Date.now() - 60_000);
    for (const file of ["lib/sidebar/model.js", "bin/tmux-sidebar"]) fs.utimesSync(path.join(home, "switchboard", file), old, old);
    fs.mkdirSync(path.join(home, ".config/tmux"), { recursive: true });
    fs.writeFileSync(path.join(home, ".config/tmux/sidebar.sh"), `#!/bin/sh\necho "$1" >> ${JSON.stringify(started)}\n`, { mode: 0o755 });

    assert.equal(run(false).stdout, "changes=0\n", "a daemon newer than its code is left alone");

    // A pull that fast-forwards the sidebar's code.
    const now = new Date(Date.now() + 5_000);
    fs.utimesSync(path.join(home, "switchboard/lib/sidebar/model.js"), now, now);
    assert.equal(run(true).stdout, "restart the sidebar daemon, for its new code\nchanges=1\n");
    assert.equal(daemon.exitCode ?? daemon.signalCode, null, "--check stops nothing");

    const exited = new Promise((resolve) => daemon.on("exit", resolve));
    assert.equal(run(false).stdout, "restart the sidebar daemon, for its new code\nchanges=1\n");
    await exited;
    assert.equal(fs.readFileSync(started, "utf8"), "start\n", "and starts it again");
    assert.equal(run(false).stdout, "changes=0\n", "a pid file naming no daemon is left alone");
  } finally {
    daemon?.kill();
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test("setup.sh's zsh checks leave the terminal to setup.sh, so the question after them can be read", () => {
  // In a terminal, zsh -i with job control makes itself the foreground, and
  // one that ends by running its command in its own place (`node --version`)
  // never gives it back: the read of the next question stopped setup.sh.
  const home = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "setup-tty-")));
  const env = { ...process.env, HOME: home, ZDOTDIR: home };
  try {
    fs.writeFileSync(path.join(home, ".zshrc"), "");
    const inZsh = fs.readFileSync(script, "utf8").match(/^in_zsh\(\) \{.*\}$/m)[0];
    const probe = `${inZsh}\nin_zsh /usr/bin/true\nps -o tpgid= -o pgid= -p $$`;
    // script(1) gives the probe a terminal of its own, as setup.sh has one.
    const result = spawnSync("script", ["-q", "/dev/null", "bash", "-c", probe], { env, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
    const groups = result.stdout.match(/(\d+)\s+(\d+)\s*$/);
    assert.ok(groups, result.stdout + result.stderr);
    assert.equal(groups[1], groups[2], "the terminal's foreground is still setup.sh's process group");
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
});
