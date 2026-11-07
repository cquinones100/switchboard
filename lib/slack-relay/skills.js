// Part of bin/slack-relay; see the comment at the top of that file.

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { tryTmux } = require("./util");
const { transcriptFor, paneFor } = require("./tabs");

// ── Skills ──────────────────────────────────────────────────────────────────

// Slack takes any message starting with "/" for one of its own commands and
// never sends it, so a skill can't be typed into a thread as it is at the
// keyboard. The tab's screen and the buttons under a finished turn offer a
// Skill form instead: a menu of the skills that tab can run, and an Arguments
// field for what follows the name, which says what the skill picked takes
// when its file has an argument-hint (see screens.js). The menu is the tab's
// own:
//
// - the skills its session was told about, which Claude Code writes into the
//   transcript as `skill_listing` attachments: the whole list when the session
//   starts, then any added since;
// - skills only you can invoke, which that list leaves out, from the skill and
//   command folders Claude Code reads: yours in ~/.claude, the plugins tmux
//   gives every pane in CLAUDE_CODE_PLUGIN_DIRS (Switchboard's own, which
//   counts as yours, as its skills did when setup linked them into ~/.claude),
//   and the project's in .claude in the tab's directory and the directories
//   above it.
//
// A Slack menu holds at most 100 options, and a project can have more skills
// than that, so each skill says where it came from: the ones you use most
// first (see FREQUENT), then yours, then the project's, then everything else
// the session lists (plugins and built-in ones), which is where a long list is
// cut. The form has a field for a name the menu left out.
//
// Claude Code's own commands are in neither place, so the ones worth running
// from Slack are listed here (BUILT_IN), after the project's: those that
// finish without asking anything. One that opens a picker (/model, /resume,
// /config) would wait on a key Slack can't press, so it isn't offered.

const SKILL_NAME = /^[\w.-]+(?::[\w.-]+)?$/;
const MAX_SKILLS = 100; // a Slack menu holds at most 100 options
const SOURCES = ["frequent", "yours", "project", "claude", "session"];
const BUILT_IN = [
  { name: "compact", description: "Summarize the conversation so far to free up context", hint: "[what the summary should keep]" },
  { name: "clear", description: "Start a new conversation in the tab", hint: "" },
  { name: "context", description: "Show how the context is used", hint: "" },
  { name: "cost", description: "Show this session's cost and duration", hint: "" },
  { name: "usage", description: "Show your plan's usage limits", hint: "" },
];

// The skills you use most, of those the tab can run, from the count and last
// use Claude Code keeps for each in ~/.claude.json (skillUsage). Each use
// counts for half as much every USE_HALF_LIFE_DAYS, so a skill you used a lot
// months ago gives way to one you use now. At most FREQUENT of them, each used
// at least MIN_USES times.
const FREQUENT = 8;
const MIN_USES = 2;
const USE_HALF_LIFE_DAYS = 30;
const usageScores = () => {
  try {
    const usage = JSON.parse(fs.readFileSync(path.join(os.homedir(), ".claude.json"), "utf8")).skillUsage ?? {};
    return new Map(
      Object.entries(usage)
        .filter(([, use]) => use.usageCount >= MIN_USES)
        .map(([name, use]) => [name, use.usageCount * 0.5 ** ((Date.now() - (use.lastUsedAt ?? 0)) / (USE_HALF_LIFE_DAYS * 86_400_000))]),
    );
  } catch {
    return new Map();
  }
};

const oneLine = (text, max) => {
  const flat = String(text ?? "").replace(/\s+/g, " ").trim();
  return flat.length > max ? `${flat.slice(0, max - 1)}…` : flat;
};

// A skill or command file's description and argument-hint (what it takes,
// such as `[ticket ID]`), from its front matter.
const describe = (file) => {
  try {
    const head = fs.readFileSync(file, "utf8").slice(0, 4000);
    const front = head.startsWith("---") ? head.slice(3, head.indexOf("\n---", 3)) : "";
    const field = (key) => front.match(new RegExp(`^${key}:\\s*(.+)$`, "m"))?.[1].replace(/^["'>|-]\s*|["']$/g, "") ?? "";
    return { description: field("description"), hint: field("argument-hint") };
  } catch {
    return { description: "", hint: "" };
  }
};

// The .claude folders that apply to a directory: its own and its parents',
// up to (not including) your home, which is read as yours.
const projectFolders = (dir) => {
  const home = os.homedir();
  const folders = [];
  for (let at = dir; at && at !== home && at !== path.dirname(at); at = path.dirname(at)) folders.push(path.join(at, ".claude"));
  return folders;
};

// What a tab can run, as [{ name, description, hint, source }]: the ones you use
// most, most used first, then yours, the project's and the rest, each sorted
// by name.
const skillsFor = (tab) => {
  const found = new Map();
  // The session's list has no argument hints; only the files do.
  const add = (name, description, source, hint = "") => {
    if (!SKILL_NAME.test(name)) return;
    const known = found.get(name);
    if (!known) found.set(name, { description: oneLine(description, 150), hint: oneLine(hint, 150), source });
    else if (!known.description) known.description = oneLine(description, 150);
  };

  const dir = tryTmux(["display-message", "-p", "-t", paneFor(tab.id), "#{pane_current_path}"]);
  const plugins = (tryTmux(["show-environment", "-g", "CLAUDE_CODE_PLUGIN_DIRS"]) ?? "").match(/^CLAUDE_CODE_PLUGIN_DIRS=(.+)$/m)?.[1].split(":").filter(Boolean) ?? [];
  [[path.join(os.homedir(), ".claude"), "yours"], ...plugins.map((folder) => [folder, "yours"]), ...(dir ? projectFolders(dir) : []).map((folder) => [folder, "project"])].forEach(([folder, source]) => {
    const read = (sub) => {
      try {
        return fs.readdirSync(path.join(folder, sub), { withFileTypes: true });
      } catch {
        return [];
      }
    };
    read("skills").forEach((entry) => {
      const skill = path.join(folder, "skills", entry.name, "SKILL.md");
      if (!fs.existsSync(skill)) return;
      const { description, hint } = describe(skill);
      add(entry.name, description, source, hint);
    });
    read("commands").filter((entry) => entry.name.endsWith(".md")).forEach((entry) => {
      const { description, hint } = describe(path.join(folder, "commands", entry.name));
      add(entry.name.slice(0, -3), description, source, hint);
    });
  });

  const file = transcriptFor(tab.id, tab.pid);
  let listed = [];
  try {
    fs.readFileSync(file, "utf8").split("\n").forEach((line) => {
      if (!line.includes('"skill_listing"')) return;
      const attachment = JSON.parse(line).attachment;
      if (attachment?.type !== "skill_listing") return;
      const described = new Map([...String(attachment.content ?? "").matchAll(/^- (\S+?)(?: \([^)\n]*\))?: (.*)$/gm)].map((m) => [m[1], m[2]]));
      const entries = (attachment.names ?? [...described.keys()]).map((name) => [name, described.get(name) ?? ""]);
      listed = attachment.isInitial ? entries : [...listed, ...entries];
    });
  } catch {}
  BUILT_IN.forEach(({ name, description, hint }) => add(name, description, "claude", hint));
  listed.forEach(([name, description]) => add(name, description, "session"));

  const scores = usageScores();
  const frequent = new Set([...found.keys()].filter((name) => scores.has(name)).sort((a, b) => scores.get(b) - scores.get(a)).slice(0, FREQUENT));
  return [...found]
    .map(([name, { description, hint, source }]) => ({ name, description, hint, source: frequent.has(name) ? "frequent" : source }))
    .sort(
      (a, b) =>
        SOURCES.indexOf(a.source) - SOURCES.indexOf(b.source) ||
        (a.source === "frequent" ? scores.get(b.name) - scores.get(a.name) : 0) ||
        a.name.localeCompare(b.name),
    );
};

module.exports = { SKILL_NAME, MAX_SKILLS, SOURCES, BUILT_IN, FREQUENT, MIN_USES, usageScores, skillsFor };
