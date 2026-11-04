// Tests for the request audit (lib/requests.js, tmux/count-request.sh) and
// its rows in mission control's Usage view. Run with: node --test test/*.test.js
//
// HOME is a temporary directory, so the counts are written there.

const { test, after } = require("node:test");
const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "requests-")));
const saved = { HOME: process.env.HOME, SWITCHBOARD_SETTINGS: process.env.SWITCHBOARD_SETTINGS };
process.env.HOME = tmp;
process.env.SWITCHBOARD_SETTINGS = path.join(tmp, "settings.json");
after(() => {
  Object.entries(saved).forEach(([key, value]) => (value === undefined ? delete process.env[key] : (process.env[key] = value)));
  fs.rmSync(tmp, { recursive: true, force: true });
});

test("requests are counted by service and protocol, from Node and from the shell, and the Usage view lists them busiest first", () => {
  const { count, tally, prune } = require("../lib/requests");
  count("GitHub", "GraphQL", "sidebar: tab PRs");
  count("GitHub", "GraphQL", "sidebar: tab PRs");
  count("Slack", "MCP", "connector");
  count("Anthropic", "claude -p", "connector");
  // A link view to a host named like one of Object's own.
  count("constructor", "HTTPS", "link view");
  // A page on this Mac is no service outside it.
  count("localhost", "HTTP", "link view");
  count("127.0.0.1", "HTTP", "link view");
  const shell = spawnSync("bash", ["-c", `. "${path.join(__dirname, "../tmux/count-request.sh")}" && count_request GitHub REST 'M-m failing log' && count_request Linear GraphQL 'tab name: ticket title'`], { env: { ...process.env, HOME: tmp }, encoding: "utf8" });
  assert.equal(shell.status, 0, shell.stderr);

  const day = tally();
  assert.equal(day.total, 7);
  assert.deepEqual(day.protocols, { GraphQL: 3, MCP: 1, "claude -p": 1, HTTPS: 1, REST: 1 });
  assert.deepEqual(day.services[0], { service: "GitHub", total: 3, protocols: { GraphQL: 2, REST: 1 } }, "the busiest service first, with its protocols");

  // A file older than the days kept goes; today's stays.
  const dir = path.join(tmp, ".local/state/switchboard/requests");
  fs.writeFileSync(path.join(dir, "2001-01-01.tsv"), "1\tGitHub\tREST\told\n");
  prune();
  assert.equal(fs.existsSync(path.join(dir, "2001-01-01.tsv")), false);
  assert.equal(tally().total, 7);

  const { usageItems } = require("../lib/sidebar/sessions");
  const rows = usageItems().map((item) => `${item.title} | ${item.detail}`);
  assert.ok(rows.includes("Requests today 7 | by protocol: GraphQL 3 · MCP 1 · claude -p 1 · HTTPS 1 · REST 1"), rows.join("\n"));
  assert.ok(rows.includes("  GitHub 3 | GraphQL 2 · REST 1"));
  assert.ok(rows.includes("  constructor 1 | HTTPS 1"), "a host named like one of Object's own");
  assert.ok(rows.some((row) => row.startsWith("  Not counted |")));

  fs.writeFileSync(process.env.SWITCHBOARD_SETTINGS, JSON.stringify({ request_audit: false }));
  require("../lib/settings").reloadSettings?.();
  assert.ok(!usageItems().some((item) => item.title.startsWith("Requests today")), "the setting off leaves them out");
});
