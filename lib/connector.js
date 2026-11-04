// One call to a tool of Claude's connectors (Notion, Google Drive, Slack),
// for scripts that can't reach them otherwise: a headless Haiku with no
// built-in tools but tool search, and only that one tool allowed, told to load
// it and call it once with the given input. Tool search keeps every other
// connector's tool definitions out of its context: with them all loaded, a
// call read about 230,000 tokens a turn and cost about $0.05, against well
// under a cent this way. The answer is the call's own result, taken
// from the stream rather than from what Haiku says, so it is what the
// connector gave. Its stdin is closed, so it doesn't wait for input, and it
// keeps no session. Claude Code's own bin is added to the PATH, for the
// sidebar daemon, which tmux starts without it. A result too large for the
// conversation is saved to a file, which is read instead. Used by
// lib/link-view.js, lib/search.js and the meeting notes
// (lib/sidebar/meetings.js).
//
//   const text = await callConnector("mcp__claude_ai_Notion__notion-fetch", { id: url });
//
// A connector that isn't connected, or whose sign-in has lapsed, fails with
// `notConnected` set and a message saying what to do, and each call records
// how its service answered in the sidebar's state directory: connectors.json
// ({ "Slack": { ok, at, message } }) for the views, and connectors-down, a
// "<service>\t<when>" line for each one down, which tmux/health.sh reads with
// builtins alone. So a feature that is on and can't reach its connector says
// so where it shows, and in the health light, rather than failing quietly.

const fs = require("node:fs");
const { count } = require("./requests");
const os = require("node:os");
const path = require("node:path");
const { execFile } = require("node:child_process");

const STATE_DIR = () => process.env.TMUX_SIDEBAR_STATE_DIR || path.join(os.homedir(), ".local/state/tmux-sidebar");
const STATUS = () => path.join(STATE_DIR(), "connectors.json");
// The service a tool belongs to, as Claude names its connector:
// mcp__claude_ai_Google_Calendar__list_events is Google Calendar.
const serviceOf = (tool) => (/^mcp__claude_ai_(.+?)__/.exec(tool)?.[1] ?? "the connector").replace(/_/g, " ");
const readStatus = () => {
  try {
    return JSON.parse(fs.readFileSync(STATUS(), "utf8"));
  } catch {
    return {};
  }
};
const record = (service, ok, message) => {
  const status = readStatus();
  if (status[service]?.ok === ok && ok) return;
  status[service] = { ok, at: Date.now(), ...(message && { message }) };
  try {
    fs.mkdirSync(STATE_DIR(), { recursive: true });
    fs.writeFileSync(STATUS(), JSON.stringify(status));
    const down = Object.entries(status).filter(([, s]) => !s.ok).map(([name, s]) => `${name}\t${Math.floor(s.at / 1000)}\n`);
    fs.writeFileSync(path.join(STATE_DIR(), "connectors-down"), down.join(""));
  } catch {}
};
// What a view shows when its service is down, or nothing.
const connectorDown = (service) => {
  const s = readStatus()[service];
  return s && !s.ok ? s.message : undefined;
};
const notConnected = (service) => {
  const err = new Error(`${service} isn't connected in Claude: run /mcp in Claude and sign in to ${service}.`);
  err.notConnected = true;
  return err;
};
// A tool's own error that means its sign-in is missing or has lapsed.
const AUTH = /\b(authenticat|unauthori[sz]ed|sign(ed)? ?in|log(ged)? ?in|re-?auth|expired token|invalid_auth|not_authed|401|403)/i;

const callConnector = (tool, input, options) => {
  const service = serviceOf(tool);
  return call(tool, input, options).then(
    (text) => {
      record(service, true);
      return text;
    },
    (err) => {
      if (err.notConnected) record(service, false, err.message);
      throw err;
    },
  );
};

const call = (tool, input, { timeout = 120_000 } = {}) =>
  new Promise((resolve, reject) => {
    const prompt = `Load ${tool} with ToolSearch (query "select:${tool}"), then call it once, with exactly this input: ${JSON.stringify(input)}. Then reply "done".`;
    // Whether Claude started at all, which a failed launch (no claude, no permission) never does.
    let started = false;
    const child = execFile(
      "claude",
      ["-p", "--model", "haiku", "--output-format", "stream-json", "--verbose", "--no-session-persistence", "--tools", "ToolSearch", "--allowedTools", `ToolSearch,${tool}`, "--max-turns", "4", prompt],
      { cwd: os.tmpdir(), env: { ...process.env, PATH: `${process.env.PATH}:${os.homedir()}/.local/bin`, ENABLE_TOOL_SEARCH: "true", MAX_MCP_OUTPUT_TOKENS: "200000" }, encoding: "utf8", maxBuffer: 64 * 1024 * 1024, timeout },
      (err, stdout) => {
        // Counted once Claude ran, and the connector once Claude called it,
        // a run that then failed or ran out of time included.
        if (started) count("Anthropic", "claude -p", "connector");
        // Each call by its own ID, a retry counted as one more.
        const calls = new Set();
        String(stdout ?? "").split("\n").forEach((line) => {
          try {
            const content = JSON.parse(line).message?.content;
            (Array.isArray(content) ? content : []).filter((p) => p.type === "tool_use" && p.name === tool).forEach((p) => calls.add(p.id));
          } catch {}
        });
        if (calls.size) count(serviceOf(tool), "MCP", tool, calls.size);
        if (err) return reject(new Error(err.code === "ENOENT" ? "Claude Code isn't installed" : `Claude didn't answer (${String(err.message).split("\n")[0]})`));
        const parts = stdout.split("\n").flatMap((line) => {
          try {
            const content = JSON.parse(line).message?.content;
            return Array.isArray(content) ? content : [];
          } catch {
            return [];
          }
        });
        const use = parts.find((p) => p.type === "tool_use" && p.name === tool);
        const result = use && parts.find((p) => p.type === "tool_result" && p.tool_use_id === use.id);
        // Never called: Claude has no such tool, because the connector isn't
        // connected (or is turned off) in Claude.
        if (!result) return reject(notConnected(serviceOf(tool)));
        const text = Array.isArray(result.content) ? result.content.map((c) => c.text ?? "").join("") : String(result.content ?? "");
        // Claude Code says where it put a result too large to show, as text
        // or as the result's content blocks in JSON; the file is read and
        // removed, being this call's alone.
        const saved = /(?:has been saved to|Full output saved to:) (\/\S+?\.(txt|json))\b/.exec(text);
        if (saved) {
          try {
            const raw = fs.readFileSync(saved[1], "utf8");
            fs.rmSync(saved[1], { force: true });
            return resolve(saved[2] === "json" ? JSON.parse(raw).map((c) => c.text ?? "").join("") : raw);
          } catch {}
        }
        if (result.is_error) return reject(AUTH.test(text) ? notConnected(serviceOf(tool)) : new Error(text.split("\n")[0] || "the connector refused"));
        resolve(text);
      },
    );
    child.once("spawn", () => (started = true));
    child.stdin.end();
  });

module.exports = { callConnector, connectorDown, serviceOf };
