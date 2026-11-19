// Switchboard's tool-call hooks, written by `switchboard setup`
// (src/setup/plugin.rs, from src/setup/agent.js). PreToolUse,
// PostToolUse and PostToolUseFailure run on every tool call, and a command
// hook starts a process each time, which a security agent that inspects
// every process made slow. So this sends them from Claude Code's own
// process to the server's socket, over HTTP (src/agent/http.rs), with the
// JSON `switchboard hook` would read and the pane it would name, waiting
// at most two seconds: the second `switchboard hook` waited on the server,
// and the moment it took to read the sessions' files first, which the
// server now reads and saying nothing when the server doesn't answer. A
// server too old to answer gets `switchboard hook` run as before.
//
// PreToolUse rides `tool.call` here, without the hook's own fields, so they
// are put back from the call and the session. The server's answer to it
// (hands-free's deny of a question box, the landing push's ask) is
// Claude Code's verdict: a deny refuses the call, and an ask is given as
// the call's `tool.check` verdict, which comes after.

const WAIT_MS = 2000;

// The asks the server answered, by the call they are for, until its check.
const asks = new Map();

async function env($) {
  const [socket, pane, helper] = await Promise.all([
    $.env.get('SWITCHBOARD'),
    $.env.get('SWITCHBOARD_PANE'),
    $.env.get('SLACK_RELAY_HELPER'),
  ]);
  return { socket, pane, quiet: !!helper };
}

// The server's answer, a JSON object for Claude Code to read, or undefined.
async function send($, event, payload) {
  const { socket, pane, quiet } = await env($);
  // The relay's own Haiku sessions inherit these hooks; their turns are
  // Slack calls, not work anyone is waiting on.
  if (quiet) return undefined;
  const body = JSON.stringify(payload);
  if (socket) {
    const headers = { 'content-type': 'application/json' };
    if (pane) headers['x-switchboard-pane'] = pane;
    try {
      const answer = await Promise.race([
        $.http.fetch(`http://switchboard/hook/${event}`, {
          method: 'POST',
          headers,
          body,
          socketPath: socket,
        }),
        $.clock.sleep(WAIT_MS).then(() => 'late'),
      ]);
      if (answer === 'late') return undefined;
      if (answer.ok) return parse(answer.text);
      if (answer.status !== 404) return undefined;
    } catch {
      // No server, or one that doesn't speak HTTP: the command below.
    }
  }
  try {
    const { stdout } = await $.process.run(['switchboard', 'hook', event], {
      stdin: body,
      timeoutMs: 5000,
    });
    return parse(stdout);
  } catch {
    return undefined;
  }
}

function parse(text) {
  try {
    const value = JSON.parse(text.trim());
    return value && typeof value === 'object' && !Array.isArray(value) ? value : undefined;
  } catch {
    return undefined;
  }
}

// PreToolUse's JSON, as Claude Code gives a command hook.
async function preToolUse($, e) {
  const { tool, tool_use_id, agentId, requestMeta, consent, ...input } = e;
  const [session_id, cwd, info] = await Promise.all([$.session.id(), $.session.cwd(), $.session.info()]);
  const payload = {
    session_id,
    transcript_path: info.transcriptPath,
    cwd,
    hook_event_name: 'PreToolUse',
    tool_name: tool,
    tool_input: input,
  };
  if (tool_use_id) payload.tool_use_id = tool_use_id;
  if (agentId) payload.agent_id = agentId;
  return payload;
}

export function register(on) {
  on('tool.call', async ($, e, next) => {
    const out = (await send($, 'PreToolUse', await preToolUse($, e)))?.hookSpecificOutput;
    const reason = out?.permissionDecisionReason ?? '';
    if (out?.permissionDecision === 'deny') return { deny: reason };
    if (out?.permissionDecision === 'ask' && e.tool_use_id) asks.set(e.tool_use_id, reason);
    try {
      return await next(e);
    } finally {
      if (e.tool_use_id) asks.delete(e.tool_use_id);
    }
  });
  on('tool.check', async ($, e, next) => {
    const verdict = await next(e);
    if (!e.tool_use_id || !asks.has(e.tool_use_id) || verdict?.decision === 'deny') return verdict;
    return { decision: 'ask', reason: asks.get(e.tool_use_id) };
  });
  on('classic.PostToolUse', async ($, e, next) => {
    await send($, 'PostToolUse', e);
    return next(e);
  });
  on('classic.PostToolUseFailure', async ($, e, next) => {
    await send($, 'PostToolUseFailure', e);
    return next(e);
  });
}
