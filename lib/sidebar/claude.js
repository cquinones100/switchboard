// Part of bin/tmux-sidebar; see the comment at the top of that file.
//
// What each Claude session in tmux is talking about, read from Claude Code's
// own files and nothing else: no processes, so it can run on every redraw.
// Claude Code writes ~/.claude/sessions/<pid>.json for each process it runs,
// naming its session, the name it was given, and its tmux pane ("main:@27.%28").
// Neither file is documented, so if one changes shape the cards quietly lose
// their text and nothing else is affected.

const fs = require("node:fs");
const { StringDecoder } = require("node:string_decoder");
const path = require("node:path");
const { CLAUDE_DIR } = require("./config");

const readJson = (file) => {
  try {
    return JSON.parse(fs.readFileSync(file, "utf8"));
  } catch {
    return undefined;
  }
};

// Session files, re-read only when they change.
const sessionCache = new Map();
const sessions = () => {
  const dir = path.join(CLAUDE_DIR, "sessions");
  let names;
  try {
    names = fs.readdirSync(dir).filter((name) => name.endsWith(".json"));
  } catch {
    return [];
  }
  const seen = new Set(names);
  [...sessionCache.keys()].filter((name) => !seen.has(name)).forEach((name) => sessionCache.delete(name));
  return names.flatMap((name) => {
    const file = path.join(dir, name);
    let mtime;
    try {
      mtime = fs.statSync(file).mtimeMs;
    } catch {
      return [];
    }
    const cached = sessionCache.get(name);
    if (cached?.mtime === mtime) return cached.session ? [cached.session] : [];
    const session = readJson(file);
    sessionCache.set(name, { mtime, session });
    return session ? [session] : [];
  });
};

// pane id -> { sessionId, name, cwd }. A conversation moved into the
// background is carried on by a job whose session file has the parked
// session's parkedJobId as its jobId; the tab shows the job's conversation, as
// agent-state.sh marks the tab with the job's state.
// A headless Claude (entrypoint sdk-cli), such as one a review loop's
// script starts, inherits the tab's TMUX_PANE and names the pane too; the
// tab's own interactive Claude wins it, so the sidebar reads that one's
// transcript, background tasks included.
const headless = (session) => Boolean(session.entrypoint?.startsWith("sdk"));
const sessionsByPane = () => {
  const all = sessions();
  const byPane = new Map();
  [...all].sort((a, b) => Number(headless(b)) - Number(headless(a))).forEach((session) => {
    const pane = /%\d+$/.exec(session.tmux ?? "")?.[0];
    if (!pane || !session.sessionId) return;
    let current = session;
    const seen = new Set([current.sessionId]);
    while (current.parkedJobId) {
      const job = all.find((other) => other.jobId === current.parkedJobId && other.sessionId && !seen.has(other.sessionId));
      if (!job) break;
      seen.add(job.sessionId);
      current = job;
    }
    const named = session.nameSource && session.nameSource !== "derived" ? session.name : undefined;
    byPane.set(pane, { sessionId: current.sessionId, cwd: current.cwd ?? session.cwd, name: named, startedAt: current.startedAt ?? session.startedAt });
  });
  return byPane;
};

// Claude Code keeps a project's transcripts in a directory named after its
// path with every other character turned into "-".
const transcriptFiles = new Map();
const transcriptFor = ({ sessionId, cwd }) => {
  const direct = cwd && path.join(CLAUDE_DIR, "projects", cwd.replace(/[^A-Za-z0-9]/g, "-"), `${sessionId}.jsonl`);
  if (direct && fs.existsSync(direct)) return direct;
  if (transcriptFiles.has(sessionId) && fs.existsSync(transcriptFiles.get(sessionId))) return transcriptFiles.get(sessionId);
  try {
    const projects = path.join(CLAUDE_DIR, "projects");
    const found = fs.readdirSync(projects).map((dir) => path.join(projects, dir, `${sessionId}.jsonl`)).find((file) => fs.existsSync(file));
    if (found) transcriptFiles.set(sessionId, found);
    return found;
  } catch {
    return undefined;
  }
};

// What a session has running in the background: commands and agents Claude
// started in the background since its process started (the tool's result
// names the task's ID) that no task notification has reported ended yet. A
// task dies with its process, so one started before that, as a resumed
// conversation's transcript shows, doesn't count. Each transcript is read
// only as far as it has grown since the last look.
//
// The skills that loop in the background are told apart by what their
// background commands run (LOOPS): while one of a loop's commands runs, the
// work is that loop, and its round is how many of them have started since
// its skill last began, so many to a round. A review loop runs codex-review
// twice a round; a CI loop watches the checks once a round, starting again
// after each fix; queue-and-clean watches the merge queue (its watch also
// reads the checks, so it is tested first). deploy-test, after a merge, waits
// for the deploy (wait-deploy.js) and records its browser test (record.js).
//
// A Monitor watch is background work too, of its own kind: it reports each
// event as a task notification and goes on, so only a notification that is
// not an event ends it (its expiry, a timeout, a failure), or a TaskStop, or
// the time its start gave ("expires in 30m") passing.
//
// A loop on a timer instead (`/loop 5m …`, or a session told to check
// something every few minutes) is a cron job Claude created (CronCreate),
// which runs until CronDelete names it, the session ends, or, for a recurring
// one, seven days pass, Claude Code's own limit; a one-off one, until it is
// past due. Either way the session is looping, and scheduled says when the
// job next runs.
//
// A self-paced `/loop` waits between its ticks on a wakeup it schedules at
// the end of each (ScheduleWakeup), with nothing running: until that wakeup
// is due, and a few minutes after (a busy session fires it late), the session
// is still looping, not idle. A wakeup with `stop` ends it, as does a tick
// that schedules none, once the last one is past due.
//
// apiError is the message of a turn an API error cut short, while nothing
// has been said since.
//
// Returns { count, loop, monitors, scheduled, apiError }: loop { kind, round } while
// one runs, monitors the running watches' descriptions, and scheduled
// { prompt, at } while a wakeup is pending, at when it is due.
const LOOPS = [
  { kind: "queue", skill: "queue-and-clean", command: /mergeQueueEntry/, perRound: 1 },
  { kind: "review", skill: "review-loop", command: /codex-review/, perRound: 2 },
  { kind: "ci", skill: "ci-loop", command: /gh pr checks/, perRound: 1 },
  { kind: "deploy", skill: "deploy-test", command: /wait-deploy\.js|deploy-test\/scripts\/record\.js/, perRound: 1 },
  // preview-test runs deploy-test's scripts on an open PR's preview, so its
  // commands are deploy-test's: this entry only marks where its rounds start.
  { kind: "deploy", skill: "preview-test", command: /wait-deploy\.js|deploy-test\/scripts\/record\.js/, perRound: 1 },
];
const WAKEUP_GRACE_MS = 5 * 60_000;
const CRON_LIFE_MS = 7 * 24 * 60 * 60_000;
const CRON_JOB = /Scheduled [\w-]* ?job (\w+)/;
// When a five-field cron expression (minute hour day month weekday, in local
// time, as CronCreate takes it) next matches after `from`, looked for a
// minute at a time over the next eight days; undefined if it never does.
const cronFields = [[0, 59], [0, 23], [1, 31], [1, 12], [0, 6]];
const cronMatches = (field, value, [low, high]) =>
  field.split(",").some((part) => {
    const [range, step = "1"] = part.split("/");
    const [a, b] = range === "*" ? [low, high] : range.split("-").map(Number);
    const end = b ?? (part.includes("/") ? high : a);
    return value >= a && value <= end && (value - a) % Number(step) === 0;
  });
const nextCron = (cron, from) => {
  const fields = String(cron).trim().split(/\s+/);
  if (fields.length !== 5) return undefined;
  const t = new Date(from);
  t.setSeconds(0, 0);
  for (let i = 0; i < 8 * 24 * 60; i++) {
    t.setMinutes(t.getMinutes() + 1);
    const values = [t.getMinutes(), t.getHours(), t.getDate(), t.getMonth() + 1, t.getDay()];
    if (values.every((value, n) => cronMatches(fields[n], value, cronFields[n]))) return t.getTime();
  }
  return undefined;
};
// A loop's prompt, short: a /loop's own command, else its first sentence.
const loopName = (prompt) => {
  const text = String(prompt).replace(/^\/loop\s+(\d+[smhd]\s+)?/, "").trim();
  return text.startsWith("/") ? text.split(/\s/)[0] : text.split(/(?<=\.)\s|\n/)[0];
};
const STARTED = /(?:Command running in background with ID|agentId): ([\w-]+)/g;
// A command that outran its timeout and was moved to the background, which
// says so at the start of its own result.
const MOVED = /^Command did not complete within its \d+s timeout and was moved to the background \(ID: ([\w-]+)\)/;
const MONITOR = /Monitor started \(task ([\w-]+), expires in (\d+)m/;
const TASK_ID = /<task-id>([\w-]+)<\/task-id>/;
// Whether a notification block ends its task. Claude Code writes a task's
// status, when it has one (completed, failed, killed), among the fields before
// its summary, and a watch's event after it, with no status. So a status before
// the summary ends the task, whatever the summary quotes; with none, an event
// keeps it going and anything else ends it.
// The fields before the summary, and before the notification closes, so a
// status-less one never takes the next one's status as its own.
const headOf = (block) => block.split("<summary>")[0].split("</task-notification>")[0];
const endsTask = (block) => headOf(block).includes("<status>") || !block.includes("<event>");
// The notifications a message or a queued command carries, each a block: the
// first <task-notification>, and each that follows a closed one, as when
// several arrive together. One that appears inside another's text (its summary
// quoting one) isn't a notification of its own. The task is the one named
// before the summary.
// Each block runs from its own start to the next notification's, so a summary
// that mentions the opening tag doesn't cut it short before its event, and one
// notification's event is never taken for another's.
const notificationBlocks = (text) => {
  const OPEN = "<task-notification>";
  const parts = text.split(OPEN);
  const starts = [];
  let at = parts[0].length;
  parts.slice(1).forEach((part, i) => {
    if (i === 0 || /<\/task-notification>(\\n|\s)*$/.test(parts[i])) starts.push(at);
    at += OPEN.length + part.length;
  });
  return starts.map((start, i) => text.slice(start + OPEN.length, starts[i + 1] ?? text.length));
};
const endedIn = (text, ended) =>
  notificationBlocks(text).forEach((block) => {
    const id = TASK_ID.exec(headOf(block))?.[1];
    if (id && endsTask(block)) ended.add(id);
  });
const loopOf = (command) => LOOPS.find((loop) => loop.command.test(command))?.kind;
const taskFiles = new Map();
const READ_CHUNK = 16 * 1024 * 1024;
const backgroundWork = (file, since = 0) => {
  if (!file) return { count: 0 };
  let size;
  try {
    size = fs.statSync(file).size;
  } catch {
    return { count: 0 };
  }
  let seen = taskFiles.get(file);
  if (!seen || size < seen.offset) seen = { offset: 0, rest: "", started: new Map(), ended: new Set(), commands: new Map(), watches: new Map(), loopAt: {}, cronCalls: new Map(), crons: new Map() };
  // Read READ_CHUNK at a time: a long session's transcript runs past the
  // longest string Node can make (about 512 MB), and reading it whole threw on
  // every refresh, which stopped the sidebar being added to new windows. The
  // decoder keeps a character split between two chunks whole.
  const fd = size > seen.offset ? fs.openSync(file, "r") : undefined;
  const decoder = new StringDecoder("utf8");
  while (fd !== undefined && seen.offset < size) {
    const chunk = Buffer.alloc(Math.min(READ_CHUNK, size - seen.offset));
    fs.readSync(fd, chunk, 0, chunk.length, seen.offset);
    seen.offset += chunk.length;
    const lines = (seen.rest + decoder.write(chunk)).split("\n");
    seen.rest = lines.pop();
    lines.forEach((line) => {
      // A turn cut short by an API error ("API Error: Connection lost
      // mid-response…"): Claude Code records it as a message of its own and
      // runs no Stop hook, so the pane can be left looking busy. It stands
      // until the conversation goes on (anything said after it, by you or
      // Claude), checked by text so the lines between aren't all parsed. A
      // subagent's lines (isSidechain) are its own, not the conversation's.
      const sidechain = line.includes('"isSidechain":true');
      if (!sidechain && line.includes('"isApiErrorMessage":true')) {
        try {
          const entry = JSON.parse(line);
          // The conversation's own message, not one nested in another record.
          if (entry.type !== "assistant" || entry.isApiErrorMessage !== true || entry.isSidechain) return;
          const text = Array.isArray(entry.message?.content) ? entry.message.content.find((p) => p.type === "text")?.text : undefined;
          seen.apiError = { at: Date.parse(entry.timestamp) || 0, text: (text || "API error").split("\n")[0] };
        } catch {}
        return;
      }
      // Parsed only while an error stands, and only a line that may say
      // something: a user or assistant entry of the conversation itself, not
      // one nested in another record (a subagent's progress).
      if (seen.apiError && !sidechain && (line.includes('"type":"user"') || line.includes('"type":"assistant"'))) {
        try {
          const entry = JSON.parse(line);
          // A slash command as typed (/cost, /status, or a skill's), its
          // output, a shell command run with ! and its output, a note Claude Code adds (isMeta) or a background task's
          // report doesn't carry the conversation on by itself: a prompt does,
          // and so does Claude's answer, which a skill gets within moments.
          const content = entry.message?.content;
          const local = typeof content === "string" && /^\s*<(local-command-|command-name>|command-message>|bash-)/.test(content);
          const report = entry.origin?.kind === "task-notification";
          if ((entry.type === "user" || entry.type === "assistant") && !entry.isSidechain && !entry.isMeta && !local && !report) seen.apiError = undefined;
        } catch {}
      }
      if (!/background with ID|moved to the background|agentId|<task-id>|run_in_background|review-loop|ci-loop|queue-and-clean|deploy-test|preview-test|"Monitor"|Monitor started|"TaskStop"|"ScheduleWakeup"|"CronCreate"|"CronDelete"|Scheduled [\w-]* ?job/.test(line)) return;
      let entry;
      try {
        entry = JSON.parse(line);
      } catch {
        return;
      }
      // A task's end, wherever Claude Code wrote it: in a user message, or,
      // in newer versions, as a queued command in an attachment entry, which
      // the conversation filter below would leave out. Ending is the same
      // whichever copy says it. Only a queued command Claude Code marks as a
      // task's notification (its commandMode and origin): a message you typed
      // mid-turn is queued the same way, marked as a prompt, and may quote one.
      const attachment = entry.attachment;
      const queued = attachment?.type === "queued_command" && (attachment.commandMode === "task-notification" || attachment.origin?.kind === "task-notification") && typeof attachment.prompt === "string" ? attachment.prompt : "";
      if (entry.type !== "user" && entry.type !== "assistant" && queued) {
        endedIn(queued, seen.ended);
        return;
      }
      // The conversation alone: Claude Code also writes copies of each request
      // (api-request-blob) with no time of their own, which would start every
      // task again at time 0, before the session, and so lose it.
      if (entry.type !== "user" && entry.type !== "assistant") return;
      const at = Date.parse(entry.timestamp) || 0;
      const content = entry.message?.content ?? "";
      const text = JSON.stringify(content);
      const parts = Array.isArray(content) ? content : [];
      // A loop's skill starting: typed as /<skill>, or run by the Skill tool.
      LOOPS.forEach((loop) => {
        if (entry.type === "user" && text.includes(`<command-name>/${loop.skill}</command-name>`)) seen.loopAt[loop.kind] = at;
      });
      parts.forEach((part) => {
        const skill = part.type === "tool_use" && part.name === "Skill" && LOOPS.find((loop) => loop.skill === part.input?.skill);
        if (skill) seen.loopAt[skill.kind] = at;
        // What each background command runs, by its tool call.
        if (part.type === "tool_use" && part.input?.run_in_background) seen.commands.set(part.id, String(part.input.command ?? part.input.prompt ?? ""));
        if (part.type === "tool_use" && part.name === "Monitor") seen.watches.set(part.id, String(part.input?.description ?? "a watch"));
        if (part.type === "tool_use" && part.name === "TaskStop" && part.input?.task_id) seen.ended.add(String(part.input.task_id));
        if (part.type === "tool_use" && part.name === "CronCreate") seen.cronCalls.set(part.id, part.input ?? {});
        if (part.type === "tool_use" && part.name === "CronDelete" && part.input?.id) seen.crons.delete(String(part.input.id));
        if (part.type === "tool_result" && seen.cronCalls.has(part.tool_use_id)) {
          const job = CRON_JOB.exec(JSON.stringify(part.content ?? ""))?.[1];
          const call = seen.cronCalls.get(part.tool_use_id);
          if (job) seen.crons.set(job, { prompt: String(call.prompt ?? ""), cron: call.cron, recurring: call.recurring !== false, at });
        }
        if (part.type === "tool_use" && part.name === "ScheduleWakeup" && entry.type === "assistant") {
          seen.wakeup = part.input?.stop ? undefined : { prompt: String(part.input?.prompt ?? ""), at: at + Number(part.input?.delaySeconds ?? 0) * 1000, set: at };
        }
      });
      if (entry.origin?.kind === "task-notification" || text.includes("<task-notification>")) {
        // A watch's events keep it going; anything else about a task ends it.
        // Each text part on its own, as written, since several notifications
        // can come as separate parts of one message.
        (typeof content === "string" ? [content] : parts.filter((part) => part.type === "text").map((part) => String(part.text ?? ""))).forEach((piece) => endedIn(piece, seen.ended));
      } else {
        // Only in the result of the call that starts one: a command's output
        // can quote these words (a search through transcripts, say), and
        // would otherwise read as a task of this session's.
        parts.filter((part) => part.type === "tool_result").forEach((part) => {
          const result = JSON.stringify(part.content ?? "");
          const command = seen.commands.get(part.tool_use_id);
          const moved = MOVED.exec(typeof part.content === "string" ? part.content : (part.content?.[0]?.text ?? ""));
          if (moved && !seen.started.has(moved[1])) seen.started.set(moved[1], { at });
          // Only a call made to run in the background: a subagent run in the
          // foreground also names its agentId, once it has finished.
          [...result.matchAll(STARTED)].filter((m) => command !== undefined && !seen.started.has(m[1])).forEach((m) => seen.started.set(m[1], { at, loop: loopOf(command) }));
          const watch = seen.watches.has(part.tool_use_id) && MONITOR.exec(result);
          if (watch) seen.started.set(watch[1], { at, monitor: seen.watches.get(part.tool_use_id), until: at + Number(watch[2]) * 60_000 });
        });
      }
    });
  }
  if (fd !== undefined) {
    fs.closeSync(fd);
    taskFiles.set(file, seen);
  }
  const now = Date.now();
  const running = [...seen.started].filter(([id, task]) => task.at >= since && !seen.ended.has(id) && !(task.until && now > task.until));
  const monitors = running.filter(([, task]) => task.monitor).map(([, task]) => task.monitor);
  const loop = LOOPS.find((l) => running.some(([, task]) => task.loop === l.kind));
  const wakeup = seen.wakeup;
  const fromCron = [...seen.crons.values()]
    .filter((job) => job.at >= since && now < job.at + CRON_LIFE_MS)
    .map((job) => ({ prompt: loopName(job.prompt), at: nextCron(job.cron, job.recurring ? now : job.at), recurring: job.recurring }))
    .filter((job) => job.at && (job.recurring || now < job.at + WAKEUP_GRACE_MS))
    .sort((a, b) => a.at - b.at)[0];
  const scheduled = wakeup && wakeup.set >= since && now < wakeup.at + WAKEUP_GRACE_MS ? { prompt: loopName(wakeup.prompt), at: wakeup.at }
    : fromCron && { prompt: fromCron.prompt, at: fromCron.at };
  const apiError = seen.apiError && seen.apiError.at >= since ? { apiError: seen.apiError.text, apiErrorAt: seen.apiError.at } : {};
  if (!loop) return { count: running.length, monitors, ...(scheduled && { scheduled }), ...apiError };
  const runs = [...seen.started.values()].filter((task) => task.loop === loop.kind && task.at >= Math.max(since, seen.loopAt[loop.kind] ?? 0)).length;
  return { count: running.length, loop: { kind: loop.kind, round: Math.max(1, Math.ceil(runs / loop.perRound)) }, monitors, ...(scheduled && { scheduled }), ...apiError };
};
const backgroundTasks = (file, since) => backgroundWork(file, since).count;

// The prompt as you typed it: a slash command as "/name args", pasted text
// unwrapped, a background task's report named as one.
const promptText = (entry) => {
  const content = entry.message?.content;
  let text;
  if (typeof content === "string") text = content;
  else if (Array.isArray(content) && !content.some((part) => part.type === "tool_result")) {
    text = content.filter((part) => part.type === "text").map((part) => part.text).join("\n");
  }
  if (!text?.trim()) return undefined;
  if (entry.origin?.kind === "task-notification") return "(a background task finished)";
  const command = /<command-name>([\s\S]*?)<\/command-name>/.exec(text);
  if (command) {
    const args = /<command-args>([\s\S]*?)<\/command-args>/.exec(text)?.[1] ?? "";
    return `${command[1].trim()} ${args.trim()}`.trim();
  }
  if (/^<local-command-|^<bash-/.test(text.trim())) return undefined;
  return text.replace(/<pasted_content\b[^>]*>\n?([\s\S]*?)\n?<\/pasted_content\b[^>]*>/g, "$1").trim();
};

// A tool call in a few words, for a turn stopped on one: a permission prompt
// or a question.
const toolText = (part) => {
  const input = part.input ?? {};
  if (part.name === "AskUserQuestion") return input.questions?.[0]?.question ?? "a question";
  const detail = input.command ?? input.file_path ?? input.path ?? input.url ?? input.pattern ?? input.description ?? "";
  return `${part.name}${detail ? `: ${String(detail).split("\n")[0]}` : ""}`;
};

// The last turn from the end of a transcript: { prompt, at, reply, tools,
// pending }, where pending is the tool call the turn is stopped on, if any.
const parseTurn = (text) => {
  const entries = text.split("\n").flatMap((line) => {
    if (!line.startsWith("{")) return [];
    try {
      const entry = JSON.parse(line);
      return entry.isSidechain || entry.isMeta ? [] : [entry];
    } catch {
      return [];
    }
  });
  const start = entries.findLastIndex((entry) => entry.type === "user" && promptText(entry));
  if (start === -1) return undefined;
  const after = entries.slice(start + 1);
  const said = [];
  const calls = new Map();
  after.forEach((entry) => {
    const parts = Array.isArray(entry.message?.content) ? entry.message.content : [];
    if (entry.type === "assistant") {
      parts.forEach((part) => {
        if (part.type === "text" && part.text.trim()) said.push(part.text.trim());
        if (part.type === "tool_use") calls.set(part.id, part);
      });
    } else if (entry.type === "user") {
      parts.filter((part) => part.type === "tool_result").forEach((part) => calls.get(part.tool_use_id) && (calls.get(part.tool_use_id).answered = true));
    }
  });
  const pending = [...calls.values()].filter((call) => !call.answered).at(-1);
  return {
    prompt: promptText(entries[start]),
    at: Date.parse(entries[start].timestamp) || undefined,
    reply: said.at(-1),
    tools: calls.size,
    pending: pending && toolText(pending),
  };
};

// Read from the end, since a transcript can be tens of megabytes: the last
// TAIL bytes, then more while no prompt turns up in them.
const TAIL = 512 * 1024;
const MAX_TAIL = 16 * 1024 * 1024;
const turnCache = new Map();
const lastTurn = (file) => {
  let stat;
  try {
    stat = fs.statSync(file);
  } catch {
    return undefined;
  }
  const cached = turnCache.get(file);
  if (cached && cached.size === stat.size && cached.mtime === stat.mtimeMs) return cached.turn;
  let turn;
  const fd = fs.openSync(file, "r");
  try {
    for (let length = TAIL; ; length *= 4) {
      const take = Math.min(length, stat.size);
      const buffer = Buffer.alloc(take);
      fs.readSync(fd, buffer, 0, take, stat.size - take);
      turn = parseTurn(buffer.toString("utf8"));
      if (turn || take === stat.size || length >= MAX_TAIL) break;
    }
  } finally {
    fs.closeSync(fd);
  }
  turnCache.set(file, { size: stat.size, mtime: stat.mtimeMs, turn });
  return turn;
};

module.exports = { nextCron, loopName, sessions, sessionsByPane, transcriptFor, backgroundTasks, backgroundWork, promptText, parseTurn, lastTurn };
