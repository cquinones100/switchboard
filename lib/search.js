#!/usr/bin/env node
// Search everything (M-S-f, tmux/search.sh): one query asked of your notes,
// your ticket tracker, GitHub and Slack at once, each source's results printed as soon as
// it answers, so fzf shows the quick ones while Slack is still looking. A line
// each: what fzf shows, a tab, then the link the result opens (a note as a
// file:// link, which the link view reads).
//
// Notes: every Markdown note in $NOTES_DIR or ~/Notes (not .trash) holding
// every word, newest first, read here and sent nowhere. Tickets: Linear's or
// Jira's issue search, as the tracker setting says (lib/tracker). GitHub: issues and pull requests in
// the repositories your tabs are in, through gh. Slack: Slack's own search
// through Claude's connector (lib/connector.js), as the link view reads a
// Notion page, taking about 15 seconds. GitHub and the tracker only with their
// settings on, Slack only with search_slack on.
//
//   search.js <query>

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { execFile } = require("node:child_process");
const { promisify } = require("node:util");
const { tracker } = require("./tracker");
const { callConnector } = require("./connector");

const LIMIT = 20;
const run = (cmd, args, opts = {}) =>
  promisify(execFile)(cmd, args, { encoding: "utf8", timeout: 30_000, maxBuffer: 64 * 1024 * 1024, ...opts }).then(
    (r) => r.stdout,
    () => undefined,
  );
const dim = (s) => `\x1b[38;5;244m${s}\x1b[0m`;
const tag = (name, color) => `\x1b[${color}m${name.padEnd(6)}\x1b[0m`;
const clean = (s) => String(s ?? "").replace(/[\t\r\n]+/g, " ").replace(/\s+/g, " ").trim();
const row = (source, color, text, detail, link) => process.stdout.write(`${tag(source, color)} ${clean(text)}${detail ? `  ${dim(clean(detail))}` : ""}\t${link}\n`);

const setting = async (key) => (await run("tmux", ["show", "-gqv", `@setting_${key}`]))?.trim() !== "off";

const notes = async (words) => {
  const dir = process.env.NOTES_DIR || path.join(os.homedir(), "Notes");
  const files = (fs.readdirSync(dir, { recursive: true }) ?? [])
    .filter((f) => f.endsWith(".md") && !f.split(path.sep).some((part) => part.startsWith(".")))
    .map((f) => path.join(dir, f));
  files
    .map((file) => ({ file, text: fs.readFileSync(file, "utf8"), time: fs.statSync(file).mtimeMs }))
    .filter(({ text }) => words.every((w) => text.toLowerCase().includes(w)))
    .sort((a, b) => b.time - a.time)
    .slice(0, LIMIT)
    .forEach(({ file, text }) => {
      const lines = text.split("\n");
      const title = lines[0].replace(/^#\s*/, "") || path.basename(file, ".md");
      const hit = lines.slice(1).find((l) => l.toLowerCase().includes(words[0])) ?? "";
      row("Note", "33", title, hit.replace(/^\s*[-*]\s*(\[[ x]\]\s*)?/, "").slice(0, 120), `file://${file.split("/").map((part) => encodeURIComponent(part).replace(/['()*!]/g, (c) => `%${c.charCodeAt(0).toString(16).toUpperCase()}`)).join("/")}`);
    });
};

const tickets = async (query) => {
  if (!(await setting("tracker"))) return;
  const provider = tracker();
  if (!provider || !(await provider.credentials())) return;
  const nodes = await provider.search(query, LIMIT).catch(() => []);
  nodes.forEach((t) => row(provider.name, "35", `${t.identifier} ${t.title}`, t.state?.name, t.url));
};

const github = async (query) => {
  if (!(await setting("github"))) return;
  const dirs = [...new Set(((await run("tmux", ["list-panes", "-a", "-F", "#{pane_current_path}"])) ?? "").split("\n").filter(Boolean))];
  const remotes = await Promise.all(dirs.map((dir) => run("git", ["-C", dir, "remote", "get-url", "origin"], { timeout: 2_000 })));
  const repos = [...new Set(remotes.map((url) => /github\.com(?::\d+)?[:/]([\w.-]+\/[\w.-]+?)(?:\.git)?\s*$/.exec(url ?? "")?.[1]).filter(Boolean))];
  if (!repos.length) return;
  const out = await run("gh", ["search", "issues", query, "--include-prs", ...repos.flatMap((r) => ["--repo", r]), "--limit", String(LIMIT), "--json", "number,title,url,state,isPullRequest,repository"]);
  (out ? JSON.parse(out) : []).forEach((i) =>
    row("GitHub", "34", `${i.repository.name}#${i.number} ${i.title}`, `${i.isPullRequest ? "PR" : "issue"}, ${i.state}`, i.url),
  );
};

const slack = async (query) => {
  if (!(await setting("search_slack"))) return;
  let text;
  try {
    text = await callConnector("mcp__claude_ai_Slack__slack_search_public_and_private", { query, limit: LIMIT });
  } catch (err) {
    // Said in the list, so a missing connector isn't mistaken for no results.
    if (err.notConnected) row("Slack", "32", err.message, "", "");
    return;
  }
  let results;
  try {
    results = JSON.parse(text).results;
  } catch {
    results = text;
  }
  String(results ?? "")
    .split(/^### Result \d+ of \d+$/m)
    .slice(1)
    .forEach((block) => {
      const field = (name) => new RegExp(`^${name}: (.*)$`, "m").exec(block)?.[1] ?? "";
      const link = /^Permalink: \[[^\]]*\]\(([^)]+)\)/m.exec(block)?.[1]?.replace(/\\\//g, "/");
      // Slack's markup as it reads: a link or a mention as its label.
      const body = (/^Text: ?\n?([\s\S]*?)(?=^Context (before|after):|^---|$(?![\s\S]))/m.exec(block)?.[1] ?? "")
        .replace(/<@\w+\|([^>]+)>/g, "@$1")
        .replace(/<(https?:[^|>]+)\|([^>]+)>/g, "$2")
        .replace(/<(https?:[^>]+)>/g, "$1");
      const where = field("Channel").replace(/ \(ID: \w+\)$/, "");
      const who = field("From").replace(/ <[^>]*>/, "").replace(/ \(ID: \w+\).*$/, "");
      if (link) row("Slack", "32", body.slice(0, 140), `${who} in ${where}, ${field("Time").slice(0, 16)}`, link);
    });
};

const query = process.argv.slice(2).join(" ").trim();
if (!query) {
  console.error("Usage: search.js <query>");
  process.exit(2);
}
const words = query.toLowerCase().split(/\s+/);
Promise.all([notes(words), tickets(query), github(query), slack(query)].map((p) => p.catch(() => {})));
