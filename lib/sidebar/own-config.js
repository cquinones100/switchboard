// Part of bin/tmux-sidebar; see the comment at the top of that file.
//
// The settings that live in your own files in ~/.config/switchboard rather
// than in settings.json, each with a row in mission control's Settings view
// (ownItems), under a category named after its file: a word, a number, a list
// or a map is typed in there (Enter, then the askModal), and anything bigger
// (the on-call channels, a file of your own overrides) opens in Neovim. A new
// file there, or a new setting in one, gets an entry in OWN_FILES.

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const OWN_DIR = () => path.join(os.homedir(), ".config/switchboard");

// kind: text, number, list (comma-separated), map (key=value, comma-separated),
// or file (opened in Neovim, the whole file).
const OWN_FILES = [
  {
    file: "claude.json",
    category: "Claude Code (claude.json)",
    rows: [{ key: "file", label: "Rules file in the repo", kind: "text", detail: "With your rules in a git repository (Your CLAUDE.md, above), the file in it that holds them (CLAUDE.md when not set); setup writes ~/.claude/CLAUDE.md again when it changes, and the repository is pulled each hour with Switchboard's update check" }],
  },
  { file: "CLAUDE.md", category: "Your files", rows: [{ label: "Your CLAUDE.md rules", kind: "file", detail: "Your own rules for Claude, when Your CLAUDE.md reads them from this local file" }] },
  {
    file: "linear.json",
    category: "Linear (linear.json)",
    rows: [
      { key: "linearWorkspace", label: "Workspace", kind: "text", detail: "Your Linear workspace's name in its URLs (linear.app/<workspace>), for ticket links" },
      { key: "linearTeam", label: "Team", kind: "text", detail: "The team key /shape files tickets in (ABC)" },
      { key: "linearLabels", label: "Labels", kind: "list", detail: "The labels /shape puts on a new ticket" },
    ],
  },
  {
    file: "jira.json",
    category: "Jira (jira.json)",
    rows: [
      { key: "jiraSite", label: "Site", kind: "text", detail: "Your Jira Cloud site (https://<yours>.atlassian.net), with the Jira tracker" },
      { key: "jiraEmail", label: "Email", kind: "text", detail: "The email of the Atlassian account your API token belongs to" },
      { key: "jiraProject", label: "Project", kind: "text", detail: "The project key /shape files tickets in (ABC)" },
      { key: "jiraLabels", label: "Labels", kind: "list", detail: "The labels /shape puts on a new ticket" },
    ],
  },
  {
    file: "browser.json",
    category: "Browser (browser.json)",
    rows: [{ key: "hosts", label: "Chrome profiles", kind: "map", detail: "The Chrome profile each site's links open in (site=Profile 2), awsapps.com for M-A's AWS login; any other site opens in the default browser" }],
  },
  {
    file: "slack-updates.json",
    category: "Slack (slack-updates.json)",
    rows: [{ key: "channels", label: "Channels", kind: "list", detail: "The channel IDs whose every message the Slack view shows, besides your DMs, mentions and threads" }],
  },
  {
    file: "slack-relay.json",
    category: "Slack relay (slack-relay.json)",
    rows: [{ key: "owner", label: "Your member ID", kind: "text", detail: "Your Slack member ID, the one person the relay takes messages from; read when the relay starts" }],
  },
  {
    file: "oncall.json",
    category: "On call (oncall.json)",
    rows: [
      { label: "Alert channels", kind: "file", detail: "The channels /oncall watches, and what in each is urgent (opens the file)" },
      { key: "rules", label: "Rules file", kind: "text", detail: "The Markdown file with your team's own on-call rules" },
      { key: "repo", label: "Repository", kind: "text", detail: "The GitHub repository (owner/name) runbooks, workflow runs and code are looked up in" },
      { key: "from", label: "Notices from", kind: "text", detail: "The name shown above each notice (oncall when not set)" },
      { key: "addressed", label: "Handled reactions", kind: "list", detail: "Your team's reaction names (no colons) for an alert that is handled" },
      { key: "taking", label: "Taking reactions", kind: "list", detail: "Your team's reaction names for an alert someone is on" },
    ],
  },
  {
    file: "sidebar.json",
    category: "Sidebar layout (sidebar.json)",
    rows: [
      { key: "width", label: "Sidebar width", kind: "number", min: 20, detail: "The columns the sidebar takes, 20 or more" },
      { key: "minWindowWidth", label: "Narrowest window", kind: "number", min: 40, detail: "The narrowest window that gets a sidebar, 40 or more" },
      { key: "cardWidth", label: "Card width", kind: "number", min: 30, detail: "Mission control's cards, in columns, 30 or more" },
      { key: "cardHeight", label: "Card height", kind: "number", min: 6, detail: "Mission control's cards, in lines, 6 or more" },
      { label: "Plugins", kind: "file", detail: "The plugins that load and their options, under \"plugins\" (opens the file)" },
    ],
  },
  { file: "trusted-dirs", category: "Your files", rows: [{ label: "Trusted directories", kind: "file", detail: "Directories whose own scripts Switchboard may run, one a line" }] },
  { file: "oncall-rules.md", category: "Your files", rows: [{ label: "On-call rules", kind: "file", detail: "Your team's own on-call rules, when oncall.json's rules names this file" }] },
  { file: "tmux.conf", category: "Your files", rows: [{ label: "tmux overrides", kind: "file", detail: "Read after Switchboard's tmux.conf, so what is here wins" }] },
  { file: "nvim.lua", category: "Your files", rows: [{ label: "Neovim overrides", kind: "file", detail: "Run after Switchboard's Neovim config" }] },
  { file: "ghostty", category: "Your files", rows: [{ label: "Ghostty overrides", kind: "file", detail: "Read last by Ghostty's config" }] },
  { file: "lazygit.yml", category: "Your files", rows: [{ label: "lazygit overrides", kind: "file", detail: "Read after Switchboard's lazygit config" }] },
];

// A file that isn't there is empty; one that is there but can't be read or
// parsed throws, so nothing writes over it.
const readOwn = (file) => {
  const full = path.join(OWN_DIR(), file);
  let link = false;
  try {
    link = fs.lstatSync(full).isSymbolicLink();
  } catch {}
  if (link && !fs.existsSync(full)) throw new Error("it is a link to nothing");
  let text;
  try {
    text = fs.readFileSync(path.join(OWN_DIR(), file), "utf8");
  } catch (err) {
    if (err.code === "ENOENT") return {};
    throw err;
  }
  const data = JSON.parse(text);
  if (!data || typeof data !== "object" || Array.isArray(data)) throw new Error("it isn't a JSON object");
  return data;
};
const readOwnSafely = (file) => {
  try {
    return { data: readOwn(file) };
  } catch (err) {
    return { data: {}, broken: err.message };
  }
};

// A value as it is typed in: what the askModal starts with, and what the row
// shows in its detail.
const typedOf = (kind, value) => {
  if (value === undefined || value === null) return "";
  if (kind === "list") return [].concat(value).join(", ");
  if (kind === "map") return Object.entries(value).map(([k, v]) => `${k}=${v}`).join(", ");
  return String(value);
};

// What is typed in, as the file keeps it; an error says what is wrong.
const parseTyped = (kind, text, min = 1) => {
  const trimmed = text.trim();
  if (!trimmed) return { remove: true };
  if (kind === "number") {
    const n = Number(trimmed);
    // Columns and lines, which tmux takes whole.
    return Number.isInteger(n) && n >= min ? { value: n } : { error: `${trimmed} isn't a whole number, ${min} or more` };
  }
  if (kind === "list") return { value: trimmed.split(",").map((s) => s.trim()).filter(Boolean) };
  if (kind === "map") {
    const pairs = trimmed.split(",").map((s) => s.trim()).filter(Boolean);
    const bad = pairs.find((p) => !/^[^=]+=.+$/.test(p));
    if (bad) return { error: `"${bad}" isn't site=profile` };
    return { value: Object.fromEntries(pairs.map((p) => [p.slice(0, p.indexOf("=")).trim(), p.slice(p.indexOf("=") + 1).trim()])) };
  }
  return { value: trimmed };
};

// Sets one key of a JSON file of yours, keeping the rest; written to a
// temporary file and renamed into place.
const writeOwn = (file, key, kind, text, min) => {
  const parsed = parseTyped(kind, text, min);
  if (parsed.error) return parsed.error;
  let data;
  try {
    data = readOwn(file);
  } catch (err) {
    return `~/.config/switchboard/${file} can't be read (${err.message}); fix it first`;
  }
  // Written into the file a link points to, keeping the link; readOwn has
  // already refused a link to nothing.
  let full = path.join(OWN_DIR(), file);
  try {
    full = fs.realpathSync(full);
  } catch {}
  if (parsed.remove) delete data[key];
  else data[key] = parsed.value;
  fs.mkdirSync(OWN_DIR(), { recursive: true });
  // Its own mode kept (a file holding a key may be 600); a new one only yours.
  let mode = 0o600;
  try {
    mode = fs.statSync(full).mode & 0o777;
  } catch {}
  fs.writeFileSync(`${full}.tmp`, JSON.stringify(data, null, 2) + "\n", { mode });
  fs.chmodSync(`${full}.tmp`, mode);
  fs.renameSync(`${full}.tmp`, full);
  return undefined;
};

const shortOf = (kind, value, exists) => {
  if (kind === "file") return exists ? "edit" : "not there";
  if (value === undefined || value === null || (Array.isArray(value) && !value.length)) return "not set";
  if (kind === "list") return `${[].concat(value).length} set`;
  if (kind === "map") return `${Object.keys(value).length} set`;
  return String(value);
};

// The Settings view's rows: a header per category, then each setting.
const ownItems = () =>
  [...new Set(OWN_FILES.map((f) => f.category))].flatMap((category) => {
    const rows = OWN_FILES.filter((f) => f.category === category).flatMap((f) => {
      const { data, broken } = f.file.endsWith(".json") ? readOwnSafely(f.file) : { data: {} };
      const exists = fs.existsSync(path.join(OWN_DIR(), f.file));
      return f.rows.map((row) => {
        const value = row.key ? data[row.key] : undefined;
        // A list with a comma inside one of its entries, or a map with one in a
        // key or value, can't be typed back as it is: it opens in Neovim.
        const tangled = (row.kind === "list" && [].concat(value ?? []).some((v) => String(v).includes(","))) || (row.kind === "map" && Object.entries(value ?? {}).some(([k, v]) => `${k}${v}`.includes(",") || k.includes("=")));
        const kind = tangled ? "file" : row.kind;
        const typed = row.key ? typedOf(row.kind, value) : "";
        return {
          title: row.label,
          detail: broken ? `~/.config/switchboard/${f.file} can't be read: ${broken}` : typed ? `${row.detail}: ${typed}` : `${row.detail} (~/.config/switchboard/${f.file})`,
          value: broken ? "unreadable" : shortOf(row.kind, value, exists),
          setting: `own:${f.file}:${row.key ?? ""}`,
          own: { file: f.file, key: row.key, kind, min: row.min, label: row.label, typed },
          state: typed || (row.kind === "file" && exists) ? "active" : "idle",
          search: `${category} ${f.file} ${row.key ?? ""} ${typed}`,
        };
      });
    });
    return [{ title: category, group: "category", count: rows.length, state: "idle", detail: "" }, ...rows];
  });

module.exports = { OWN_DIR, OWN_FILES, ownItems, writeOwn, parseTyped };
