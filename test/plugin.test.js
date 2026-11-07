// Tests for claude/plugin, Switchboard as a Claude Code plugin that only its
// tmux panes load (CLAUDE_CODE_PLUGIN_DIRS), and for how setup.sh and
// tmux/settings.sh move an install to it. Run with: node --test test/*.test.js
//
// Each test has a temporary HOME whose ~/switchboard links to this clone, and
// the tmux one a throwaway server, so nothing reads or changes your own.

const { test } = require("node:test");
const assert = require("node:assert/strict");
const { spawnSync, execFileSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const repo = path.join(__dirname, "..");
const plugin = path.join(repo, "claude/plugin");
const script = path.join(repo, "setup.sh");
const commandsOf = (hooks) => Object.values(hooks).flatMap((groups) => groups.flatMap((group) => group.hooks.map((hook) => hook.command)));

const withHome = (prefix, fn) => {
  const home = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), prefix)));
  try {
    fs.symlinkSync(repo, path.join(home, "switchboard"));
    fs.mkdirSync(path.join(home, ".claude"), { recursive: true });
    fs.mkdirSync(path.join(home, ".config/switchboard"), { recursive: true });
    return fn(home, { ...process.env, HOME: home, SWITCHBOARD_SETTINGS: "" });
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
};
const inUse = (env) => spawnSync("bash", [path.join(plugin, "in-use.sh")], { env }).status === 0;

test("the plugin's manifest parses, its hooks are switchboard-settings.json's and the instructions', and it holds every skill", () => {
  const manifest = JSON.parse(fs.readFileSync(path.join(plugin, ".claude-plugin/plugin.json"), "utf8"));
  assert.equal(manifest.name, "switchboard");
  const hooks = JSON.parse(fs.readFileSync(path.join(plugin, "hooks/hooks.json"), "utf8")).hooks;
  const settings = JSON.parse(fs.readFileSync(path.join(repo, "claude/switchboard-settings.json"), "utf8")).hooks;
  assert.deepEqual(commandsOf(hooks).sort(), [...commandsOf(settings), 'bash "${CLAUDE_PLUGIN_ROOT}/hooks/instructions.sh"'].sort());
  assert.deepEqual(Object.keys(hooks).sort(), Object.keys(settings).sort());
  assert.deepEqual(fs.readdirSync(path.join(plugin, "skills")).sort(), fs.readdirSync(path.join(repo, "claude/skills")).sort());
  fs.readdirSync(path.join(repo, "claude/skills")).forEach((name) => assert.ok(fs.existsSync(path.join(plugin, "skills", name, "SKILL.md")), name));
});

test("instructions.sh hands an interactive session switchboard.md, and a claude -p run nothing", () => {
  const run = (entrypoint) => execFileSync("bash", [path.join(plugin, "hooks/instructions.sh")], { env: { ...process.env, CLAUDE_CODE_ENTRYPOINT: entrypoint }, encoding: "utf8" });
  const output = JSON.parse(run("cli")).hookSpecificOutput;
  assert.equal(output.hookEventName, "SessionStart");
  assert.equal(output.additionalContext, fs.readFileSync(path.join(repo, "claude/switchboard.md"), "utf8"));
  assert.equal(run("sdk-cli"), "");
});

test("in-use.sh: a new install loads the plugin, one from before only once it says yes, and never with the setting off", () => {
  withHome("plugin-in-use-", (home, env) => {
    assert.ok(inUse(env), "nothing of the old way: in use");
    fs.writeFileSync(path.join(home, ".claude/settings.json"), JSON.stringify({ hooks: { Stop: [{ hooks: [{ type: "command", command: "bash ~/.config/tmux/agent-state.sh set done" }] }] } }));
    assert.ok(!inUse(env), "Switchboard's hooks in settings.json: not until asked");
    fs.mkdirSync(path.join(home, ".local/state/switchboard"), { recursive: true });
    fs.writeFileSync(path.join(home, ".local/state/switchboard/claude-plugin"), "moved\n");
    assert.ok(inUse(env), "said yes");
    fs.mkdirSync(path.join(home, "old-claude"));
    fs.writeFileSync(path.join(home, "old-claude/claude"), "#!/bin/sh\necho \"2.1.100 (Claude Code)\"\n", { mode: 0o755 });
    assert.ok(!inUse({ ...env, PATH: `${path.join(home, "old-claude")}:${process.env.PATH}` }), "a Claude Code too old for the variable keeps the old way");
    fs.writeFileSync(path.join(home, ".config/switchboard/settings.json"), JSON.stringify({ claude_plugin: false }));
    assert.ok(!inUse(env), "the setting off");
  });
});

test("setup.sh plugin moves an install from before out of ~/.claude, keeping what is yours, once", () => {
  withHome("plugin-move-", (home, env) => {
    const skills = path.join(home, ".claude/skills");
    const shipped = fs.readdirSync(path.join(repo, "claude/skills"));
    const product = JSON.parse(fs.readFileSync(path.join(repo, "claude/switchboard-settings.json"), "utf8"));
    // The old way, as setup wrote it, beside a hook, a rule and a skill of your own.
    fs.writeFileSync(path.join(home, ".config/switchboard/settings.json"), JSON.stringify({ claude_plugin: false }));
    fs.writeFileSync(path.join(home, ".claude/settings.json"), JSON.stringify({ model: "sonnet", hooks: { Stop: [{ hooks: [{ type: "command", command: "say done" }] }] } }));
    fs.writeFileSync(path.join(home, ".claude/CLAUDE.md"), "# Mine\n");
    fs.mkdirSync(path.join(skills, "wt"), { recursive: true });
    fs.writeFileSync(path.join(skills, "wt/SKILL.md"), "mine\n");
    const before = spawnSync("bash", [script, "plugin"], { env, encoding: "utf8" });
    assert.equal(before.status, 0, before.stdout + before.stderr);
    assert.match(fs.readFileSync(path.join(home, ".claude/settings.json"), "utf8"), /agent-state\.sh/);
    assert.match(fs.readFileSync(path.join(home, ".claude/CLAUDE.md"), "utf8"), /^@~\/switchboard\/claude\/switchboard\.md$/m);
    assert.equal(fs.readlinkSync(path.join(skills, "review-loop")), path.join(home, "switchboard/claude/skills/review-loop"));

    // Turned on again, which is a yes: the setting back at its default.
    fs.writeFileSync(path.join(home, ".config/switchboard/settings.json"), "{}");
    assert.ok(!inUse(env), "not before the yes");
    fs.mkdirSync(path.join(home, ".local/state/switchboard"), { recursive: true });
    fs.writeFileSync(path.join(home, ".local/state/switchboard/claude-plugin"), "moved\n");
    const check = spawnSync("bash", [script, "--check", "plugin"], { env, encoding: "utf8" });
    assert.match(check.stdout, /would: take Switchboard's hooks out/);
    assert.match(fs.readFileSync(path.join(home, ".claude/settings.json"), "utf8"), /agent-state\.sh/, "--check changes nothing");
    const move = spawnSync("bash", [script, "plugin"], { env, encoding: "utf8" });
    assert.equal(move.status, 0, move.stdout + move.stderr);

    const settings = JSON.parse(fs.readFileSync(path.join(home, ".claude/settings.json"), "utf8"));
    assert.deepEqual(commandsOf(settings.hooks), ["say done"], "only your own hook is left");
    assert.equal(settings.model, "sonnet");
    assert.deepEqual(settings.statusLine, product.statusLine, "the statusline stays");
    product.permissions.allow.forEach((rule) => assert.ok(settings.permissions.allow.includes(rule), rule));
    assert.equal(fs.readFileSync(path.join(home, ".claude/CLAUDE.md"), "utf8"), "# Mine\n\n", "your rules stay, without the import");
    shipped.filter((name) => name !== "wt").forEach((name) => assert.ok(!fs.existsSync(path.join(skills, name)) && !isLink(path.join(skills, name)), name));
    assert.equal(fs.readFileSync(path.join(skills, "wt/SKILL.md"), "utf8"), "mine\n", "your own skill stays");
    assert.equal(fs.readlinkSync(path.join(home, ".local/bin/skill-path")), path.join(home, "switchboard/bin/skill-path"), "skill-path, which every skill runs, still linked");

    const again = spawnSync("bash", [script, "plugin"], { env, encoding: "utf8" });
    assert.equal(again.stdout, "plugin: up to date\n", "a second run changes nothing");
    assert.equal(spawnSync("bash", [script, "claude"], { env, encoding: "utf8" }).status, 0);
    assert.ok(!isLink(path.join(skills, "review-loop")), "the whole setup links no skill while the plugin brings them");
    assert.doesNotMatch(fs.readFileSync(path.join(home, ".claude/settings.json"), "utf8"), /agent-state\.sh/);
  });
});

test("setup.sh plugin, turned off, writes the hooks and links the skills even when your rules file is missing", () => {
  withHome("plugin-partial-", (home, env) => {
    fs.writeFileSync(path.join(home, ".config/switchboard/settings.json"), JSON.stringify({ claude_plugin: false }));
    fs.writeFileSync(path.join(home, ".config/switchboard/claude.json"), JSON.stringify({ path: "~/gone.md" }));
    const result = spawnSync("bash", [script, "plugin"], { env, encoding: "utf8" });
    assert.equal(result.status, 1, "it fails, saying why");
    assert.match(result.stderr, /your rules aren't at ~\/gone\.md/);
    assert.match(fs.readFileSync(path.join(home, ".claude/settings.json"), "utf8"), /agent-state\.sh/);
    assert.equal(fs.readlinkSync(path.join(home, ".claude/skills/review-loop")), path.join(home, "switchboard/claude/skills/review-loop"), "the skills linked all the same");
  });
});

test("setup.sh claude on a new install writes only the statusline and permissions, and your rules' import alone", () => {
  withHome("plugin-new-", (home, env) => {
    fs.writeFileSync(path.join(home, ".config/switchboard/CLAUDE.md"), "# Rules\n");
    const result = spawnSync("bash", [script, "claude"], { env, encoding: "utf8" });
    assert.equal(result.status, 0, result.stdout + result.stderr);
    const settings = JSON.parse(fs.readFileSync(path.join(home, ".claude/settings.json"), "utf8"));
    assert.equal(settings.hooks, undefined, "no hooks: the plugin brings them");
    assert.ok(settings.statusLine);
    assert.doesNotMatch(fs.readFileSync(path.join(home, ".claude/CLAUDE.md"), "utf8"), /switchboard\.md/);
    assert.match(fs.readFileSync(path.join(home, ".claude/CLAUDE.md"), "utf8"), /^@~\/\.config\/switchboard\/CLAUDE\.md$/m);
    assert.ok(inUse(env), "and stays a plugin install");
    assert.equal(spawnSync("bash", [script, "claude"], { env, encoding: "utf8" }).stdout, "claude: up to date\n");
  });
});

test("settings.sh points every pane tmux starts at the plugin while it is in use, and nowhere with the setting off", () => {
  withHome("plugin-tmux-", (home, env) => {
    const socket = path.join(home, "tmux.sock");
    execFileSync("tmux", ["-S", socket, "-f", "/dev/null", "new-session", "-d", "-s", "plugin", "cat"]);
    try {
      const pid = execFileSync("tmux", ["-S", socket, "display", "-p", "#{pid}"], { encoding: "utf8" }).trim();
      const tmuxEnv = { ...env, TMUX: `${socket},${pid},0`, SWITCHBOARD_DIR: repo };
      const shown = () => spawnSync("tmux", ["-S", socket, "show-environment", "-g", "CLAUDE_CODE_PLUGIN_DIRS"], { encoding: "utf8" }).stdout.trim();
      execFileSync("bash", [path.join(repo, "tmux/settings.sh"), "apply", "--load"], { env: tmuxEnv });
      assert.equal(shown(), `CLAUDE_CODE_PLUGIN_DIRS=${plugin}`);
      fs.writeFileSync(path.join(home, ".config/switchboard/settings.json"), JSON.stringify({ claude_plugin: false }));
      execFileSync("bash", [path.join(repo, "tmux/settings.sh"), "apply", "--load"], { env: tmuxEnv });
      assert.doesNotMatch(shown(), /^CLAUDE_CODE_PLUGIN_DIRS=/);
      // Off with nothing of the old way in ~/.claude (the file changed while
      // tmux was down): setup writes it there, in the background.
      const written = () => fs.existsSync(path.join(home, ".claude/settings.json")) && fs.readFileSync(path.join(home, ".claude/settings.json"), "utf8").includes("agent-state.sh");
      for (let waited = 0; !written() && waited < 150; waited++) spawnSync("sleep", ["0.1"]);
      assert.ok(written(), "Switchboard's hooks written into ~/.claude/settings.json");
      // Turned on again in Settings, a yes, as settings.sh records it.
      fs.mkdirSync(path.join(home, ".local/state/switchboard"), { recursive: true });
      fs.writeFileSync(path.join(home, ".local/state/switchboard/claude-plugin"), "moved\n");
      // Another plugin folder tmux was started with stays, whichever way.
      execFileSync("tmux", ["-S", socket, "set-environment", "-g", "CLAUDE_CODE_PLUGIN_DIRS", "/other"]);
      execFileSync("bash", [path.join(repo, "tmux/settings.sh"), "apply", "--load"], { env: tmuxEnv });
      assert.equal(shown(), "CLAUDE_CODE_PLUGIN_DIRS=/other");
      fs.writeFileSync(path.join(home, ".config/switchboard/settings.json"), "{}");
      execFileSync("bash", [path.join(repo, "tmux/settings.sh"), "apply", "--load"], { env: tmuxEnv });
      assert.equal(shown(), `CLAUDE_CODE_PLUGIN_DIRS=${plugin}:/other`);
      execFileSync("bash", [path.join(repo, "tmux/settings.sh"), "apply", "--load"], { env: tmuxEnv });
      assert.equal(shown(), `CLAUDE_CODE_PLUGIN_DIRS=${plugin}:/other`, "and applying again changes nothing");
      // And setup has taken the old way out again, in the background.
      for (let waited = 0; written() && waited < 150; waited++) spawnSync("sleep", ["0.1"]);
      assert.ok(!written(), "Switchboard's hooks taken out of ~/.claude/settings.json again");
    } finally {
      spawnSync("tmux", ["-S", socket, "kill-server"]);
    }
  });
});

test("tmux starting with the setting off writes the hooks back after a move left half done, the old import still there", () => {
  withHome("plugin-half-", (home, env) => {
    const socket = path.join(home, "tmux.sock");
    execFileSync("tmux", ["-S", socket, "-f", "/dev/null", "new-session", "-d", "-s", "plugin", "cat"]);
    try {
      const pid = execFileSync("tmux", ["-S", socket, "display", "-p", "#{pid}"], { encoding: "utf8" }).trim();
      fs.writeFileSync(path.join(home, ".claude/CLAUDE.md"), "@~/switchboard/claude/switchboard.md\n");
      fs.writeFileSync(path.join(home, ".config/switchboard/settings.json"), JSON.stringify({ claude_plugin: false }));
      execFileSync("bash", [path.join(repo, "tmux/settings.sh"), "apply", "--load"], { env: { ...env, TMUX: `${socket},${pid},0`, SWITCHBOARD_DIR: repo } });
      const written = () => fs.existsSync(path.join(home, ".claude/settings.json")) && fs.readFileSync(path.join(home, ".claude/settings.json"), "utf8").includes("agent-state.sh");
      for (let waited = 0; !written() && waited < 150; waited++) spawnSync("sleep", ["0.1"]);
      assert.ok(written(), "Switchboard's hooks written back");
      for (let waited = 0; !isLink(path.join(home, ".claude/skills/wtn")) && waited < 50; waited++) spawnSync("sleep", ["0.1"]);
      assert.ok(isLink(path.join(home, ".claude/skills/wtn")), "and its skills linked");
    } finally {
      spawnSync("tmux", ["-S", socket, "kill-server"]);
    }
  });
});

test("tmux starting on a new install with an older Claude Code writes nothing into ~/.claude, leaving it to the guided setup to ask", () => {
  withHome("plugin-old-new-", (home, env) => {
    const socket = path.join(home, "tmux.sock");
    execFileSync("tmux", ["-S", socket, "-f", "/dev/null", "new-session", "-d", "-s", "plugin", "cat"]);
    try {
      const pid = execFileSync("tmux", ["-S", socket, "display", "-p", "#{pid}"], { encoding: "utf8" }).trim();
      fs.mkdirSync(path.join(home, "old-claude"));
      fs.writeFileSync(path.join(home, "old-claude/claude"), "#!/bin/sh\necho \"2.1.100 (Claude Code)\"\n", { mode: 0o755 });
      execFileSync("bash", [path.join(repo, "tmux/settings.sh"), "apply", "--load"], { env: { ...env, PATH: `${path.join(home, "old-claude")}:${process.env.PATH}`, TMUX: `${socket},${pid},0`, SWITCHBOARD_DIR: repo } });
      spawnSync("sleep", ["2"]);
      assert.ok(!fs.existsSync(path.join(home, ".claude/settings.json")), "no settings written without asking");
      assert.ok(!fs.existsSync(path.join(home, ".claude/skills")), "nor skills linked");
    } finally {
      spawnSync("tmux", ["-S", socket, "kill-server"]);
    }
  });
});

test("claude from a pane takes Switchboard's plugin as tmux has it now, keeping any other plugin folder", () => {
  withHome("plugin-shell-", (home) => {
    fs.mkdirSync(path.join(home, "bin"));
    fs.writeFileSync(path.join(home, "bin/tmux"), "#!/bin/sh\necho \"$FAKE_TMUX\"\n", { mode: 0o755 });
    fs.writeFileSync(path.join(home, "bin/claude"), "#!/bin/sh\necho \"${CLAUDE_CODE_PLUGIN_DIRS-unset}\"\n", { mode: 0o755 });
    const ours = path.join(home, "switchboard/claude/plugin");
    const run = (tmuxSays, before) =>
      execFileSync("zsh", ["-f", "-c", `eval "$(${path.join(repo, "bin/worktree-from-ticket")} init zsh)"; claude`], {
        env: { PATH: `${path.join(home, "bin")}:${process.env.PATH}`, HOME: home, TMUX: "x", FAKE_TMUX: tmuxSays, ...(before === undefined ? {} : { CLAUDE_CODE_PLUGIN_DIRS: before }) },
        encoding: "utf8",
      }).trim();
    assert.equal(run(`CLAUDE_CODE_PLUGIN_DIRS=${ours}`), ours, "moved after the shell started");
    assert.equal(run(`CLAUDE_CODE_PLUGIN_DIRS=${ours}`, `/other:${ours}`), `${ours}:/other`);
    assert.equal(run("-CLAUDE_CODE_PLUGIN_DIRS", ours), "unset", "turned off after the shell started");
    assert.equal(run("-CLAUDE_CODE_PLUGIN_DIRS", `/other:${ours}`), "/other");
    assert.equal(run(`CLAUDE_CODE_PLUGIN_DIRS=${ours}:/other`, `${ours}:/other`), `${ours}:/other`, "a folder in both is kept once");
  });
});

test("Claude Code loads the plugin from CLAUDE_CODE_PLUGIN_DIRS with every skill and hook", { skip: spawnSync("claude", ["--version"]).status !== 0 && "Claude Code isn't installed" }, () => {
  const details = execFileSync("claude", ["plugin", "details", "switchboard"], { env: { ...process.env, CLAUDE_CODE_PLUGIN_DIRS: plugin }, encoding: "utf8", cwd: os.tmpdir() });
  const skills = details.match(/Skills \((\d+)\)/)?.[1];
  assert.equal(Number(skills), fs.readdirSync(path.join(repo, "claude/skills")).length, details);
  ["PreToolUse", "SessionStart", "Stop", "Notification"].forEach((event) => assert.match(details, new RegExp(event)));
});

function isLink(file) {
  try {
    return fs.lstatSync(file).isSymbolicLink();
  } catch {
    return false;
  }
}
