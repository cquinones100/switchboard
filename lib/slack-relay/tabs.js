// Part of bin/slack-relay; see the comment at the top of that file.

const { execFileSync } = require("node:child_process");
const crypto = require("node:crypto");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { SCREEN_LINES, VERSION } = require("./config");
const { tmux, tryTmux } = require("./util");

// ── tmux ────────────────────────────────────────────────────────────────────

// One row per window that has a Claude pane: its number, name (whole, from the
// @tab_label window-names.sh keeps, as the tab itself is cut short), and its Claude
// pane, the one the relay reads, follows and types into. A window can hold
// other panes beside Claude (a shell, a server, another Claude), so a tab
// keeps the pane it had: the first Claude pane the relay sees in a window is
// recorded on the window as @claude_pane (a tab started with `new:` records
// its pane when it is made), and stays the tab's pane for as long as it is
// open, through /clear or Claude being started again in it. While it isn't
// running Claude, the window's first other Claude pane stands in, without
// being recorded; once it has closed, that pane is recorded instead. The row
// has that pane's state and since when, its id (pane), its process (pid),
// which transcriptFor would otherwise ask tmux for again, every Claude pane's
// process (pids), and whether the window is the tab in front of you (the
// active window of an attached session), where agent-state.sh never marks a
// turn done. Each call also notes every window's Claude pane for paneFor.
const CLAUDE_PANE = "@claude_pane";

// Claude Code started from a Node script, as claude-here starts it, runs as
// the script's child in the terminal's foreground, and tmux names the pane
// after node. Such a pane is Claude's while a foreground process on its
// terminal has a session file, which only Claude Code writes. That takes a ps,
// so the answer is kept for each pane process for WRAPPED_CACHE_MS.
const WRAPPED_CACHE_MS = 10_000;
const wrapped = new Map();
const wrapsClaude = (pid, tty) => {
  const cached = wrapped.get(pid);
  if (cached && Date.now() - cached.at < WRAPPED_CACHE_MS) return cached.claude;
  let claude = false;
  try {
    claude = execFileSync("ps", ["-t", tty.replace(/^\/dev\//, ""), "-o", "pid=,stat="], { encoding: "utf8" })
      .split("\n")
      .map((line) => line.trim().split(/\s+/))
      .some(([foreground, stat]) => stat?.includes("+") && fs.existsSync(path.join(os.homedir(), ".claude/sessions", `${foreground}.json`)));
  } catch {}
  wrapped.set(pid, { claude, at: Date.now() });
  return claude;
};
const claudePanes = new Map();
const claudeTabs = () => {
  const out = tryTmux([
    "list-panes", "-a", "-F",
    `#{window_id}\t#{window_index}\t#{window_name}\t#{pane_current_command}\t#{@agent_state}\t#{@agent_since}\t#{@slack_relay}\t#{pane_pid}\t#{&&:#{window_active},#{session_attached}}\t#{pane_id}\t#{pane_index}\t#{${CLAUDE_PANE}}\t#{pane_tty}\t#{@tab_label}\t#{@loop_tab}`,
  ]);
  if (!out) return [];
  const windows = new Map();
  out.split("\n").forEach((line) => {
    const [id, index, name, command, state, since, relay, pid, focused, pane, paneIndex, recorded, tty, fullName, loop] = line.split("\t");
    if (!windows.has(id)) windows.set(id, { id, index: Number(index), name: fullName || name, focused: focused === "1", recorded, relay: false, loop: loop === "1", panes: [] });
    const window = windows.get(id);
    if (relay === "1") window.relay = true;
    window.panes.push({ pane, pid, state: state || "idle", since, claude: VERSION.test(command) || (command === "node" && wrapsClaude(pid, tty)), paneIndex: Number(paneIndex) });
  });
  const tabs = [];
  windows.forEach((window) => {
    const claude = window.panes.filter((p) => p.claude).sort((a, b) => a.paneIndex - b.paneIndex);
    if (window.relay || !claude.length) return;
    const recorded = window.panes.find((p) => p.pane === window.recorded);
    const chosen = recorded?.claude ? recorded : claude[0];
    if (!recorded) tryTmux(["set-option", "-w", "-t", window.id, CLAUDE_PANE, chosen.pane]);
    tabs.push({
      id: window.id, index: window.index, name: window.name, state: chosen.state, since: chosen.since, focused: window.focused, loop: window.loop,
      pane: chosen.pane, pid: chosen.pid, pids: claude.map((p) => p.pid),
    });
  });
  claudePanes.clear();
  tabs.forEach((tab) => claudePanes.set(tab.id, tab.pane));
  return tabs.sort((a, b) => a.index - b.index);
};

// The pane to read or type into for a window: its Claude pane, as the last
// claudeTabs saw it (the relay calls it every second), else the window
// itself, which tmux takes as its active pane.
const paneFor = (windowId) => {
  if (!String(windowId).startsWith("@")) return windowId;
  if (!claudePanes.has(windowId)) claudeTabs();
  return claudePanes.get(windowId) ?? windowId;
};

const label = (tab) => `tab ${tab.index}, ${tab.name}`;

// How a tab's state reads in Slack, colored so the one waiting on you stands
// out in a list.
const STATE_WORDS = { waiting: "🔴 **waiting on you**", done: "✅ finished", working: "🔵 working", idle: "⚪ idle" };

// A Markdown table, which Slack's markdown blocks render as a real table.
// Each cell is one line, with any | in it escaped.
const cell = (text) => String(text ?? "").replace(/\s+/g, " ").replace(/\|/g, "\\|").trim();
const table = (headers, rows) =>
  [`| ${headers.join(" | ")} |`, `|${headers.map(() => "---").join("|")}|`, ...rows.map((row) => `| ${row.map(cell).join(" | ")} |`)].join("\n");

// The window's Claude pane, to type into, looked up afresh: whichever pane
// you are using, typing goes to Claude, never to a shell beside it.
const claudePane = (windowId) => {
  const tab = claudeTabs().find((t) => t.id === windowId);
  if (tab) return { pane: tab.pane };
  const command = tryTmux(["display-message", "-p", "-t", windowId, "#{pane_current_command}"]);
  if (command === undefined) return { error: "That tab no longer exists." };
  return { error: `That tab is running ${command}, not Claude, so nothing was sent.` };
};

// The Claude conversation running in a window, as its transcript file: the
// window's Claude pane runs it (directly, under its shell, or under a wrapper
// like claude-here), which records
// its session in ~/.claude/sessions/<pid>.json, and the transcript is
// ~/.claude/projects/<dir>/<session>.jsonl. Undefined when any step is missing.
const isAlive = (pid) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch (err) {
    return err.code === "EPERM";
  }
};

// The relay asks every second, so an answer is kept for its pane process
// while every session file it was read from is unchanged, and for at most
// TRANSCRIPT_CACHE_MS: finding it takes a pgrep and a look through every
// project directory. A session that ends removes its file, and /clear or
// parking rewrites it, so a stale answer doesn't outlive either.
const TRANSCRIPT_CACHE_MS = 30_000;
const transcripts = new Map();
const mtime = (file) => {
  try {
    return fs.statSync(file).mtimeMs;
  } catch {
    return undefined;
  }
};

const transcriptFor = (windowId, panePid) => {
  const pid = panePid ?? tryTmux(["display-message", "-p", "-t", paneFor(windowId), "#{pane_pid}"]);
  if (!pid) return undefined;
  const cached = transcripts.get(pid);
  if (
    cached &&
    Date.now() - cached.at < TRANSCRIPT_CACHE_MS &&
    cached.sources.every(([file, time]) => mtime(file) === time) &&
    fs.existsSync(cached.file)
  ) {
    return cached.file;
  }
  transcripts.delete(pid);
  const file = findTranscript(pid);
  if (file) transcripts.set(pid, { ...file, at: Date.now() });
  return file?.file;
};

// { file, sources }: the transcript, and [path, mtime] for each session file
// that led to it.
const findTranscript = (pid) => {
  // Every process under the pane's, each before its own, so Claude under a
  // wrapper is found as well as Claude straight under the shell.
  let descendants = [];
  try {
    const rows = execFileSync("ps", ["-A", "-o", "pid=,ppid="], { encoding: "utf8" })
      .split("\n")
      .map((line) => line.trim().split(/\s+/))
      .filter(([child]) => child);
    const under = (parent) => rows.filter(([, ppid]) => ppid === parent).flatMap(([child]) => [child, ...under(child)]);
    descendants = under(String(pid));
  } catch {}
  const sessionsDir = path.join(os.homedir(), ".claude/sessions");
  const sessionFile = [pid, ...descendants].map((p) => path.join(sessionsDir, `${p}.json`)).find((p) => fs.existsSync(p));
  if (!sessionFile) return undefined;
  try {
    const sources = [[sessionFile, mtime(sessionFile)]];
    let session = JSON.parse(fs.readFileSync(sessionFile, "utf8"));
    // A session moved into the background is "parked": its file names the
    // background job that carries the conversation on (parkedJobId), and that
    // job's own session file has the same jobId. The conversation the tab is
    // part of is the job's, so its transcript is the one to read. The job can
    // itself have been parked, so the links are followed to the end, each
    // session at most once.
    if (session.parkedJobId) {
      const sessions = fs.readdirSync(sessionsDir)
        .filter((name) => name.endsWith(".json"))
        .flatMap((name) => {
          try {
            const file = path.join(sessionsDir, name);
            return [{ ...JSON.parse(fs.readFileSync(file, "utf8")), file }];
          } catch {
            return [];
          }
        });
      const seen = new Set([session.sessionId]);
      let job;
      while (
        session.parkedJobId &&
        (job = sessions.find((other) => other.jobId === session.parkedJobId && other.sessionId && !seen.has(other.sessionId) && isAlive(other.pid)))
      ) {
        seen.add(job.sessionId);
        sources.push([job.file, mtime(job.file)]);
        session = job;
      }
    }
    const { sessionId } = session;
    const projects = path.join(os.homedir(), ".claude/projects");
    const file = fs.readdirSync(projects)
      .map((dir) => path.join(projects, dir, `${sessionId}.jsonl`))
      .find((candidate) => fs.existsSync(candidate));
    return file && { file, sources };
  } catch {
    return undefined;
  }
};

// How full a tab's context is, and its model: { session, percent, model },
// from the file claude/statusline-command.sh writes for the sidebar,
// ~/.local/state/tmux-sidebar/context/<session id>, holding
// "<percent>\t<model>". Claude Code gives them only to the statusline, so the
// figure is as of its last redraw, and there is none until it has drawn once.
const contextOf = (windowId, panePid) => {
  const file = transcriptFor(windowId, panePid);
  if (!file) return undefined;
  const session = path.basename(file, ".jsonl");
  try {
    const [percent, model] = fs.readFileSync(path.join(os.homedir(), ".local/state/tmux-sidebar/context", session), "utf8").split("\t");
    return /^\d+$/.test(percent) ? { session, percent: Number(percent), model: (model ?? "").trim() } : undefined;
  } catch {
    return undefined;
  }
};
const contextWords = (context) => (context ? `${context.percent}% context used` : "");

const relative = (ms) => {
  const minutes = Math.round(ms / 60_000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes}m ago`;
  if (minutes < 48 * 60) return `${Math.round(minutes / 60)}h ago`;
  return `${Math.round(minutes / 1440)} days ago`;
};

// The last turn of a tab's conversation: the user's last prompt and everything
// Claude has written since, with its tool calls counted rather than shown.
// Falls back to the screen when the transcript can't be found.
const lastTurn = (windowId, expected) => {
  const asScreen = () => `\`\`\`\n${screen(windowId) || "(empty)"}\n\`\`\``;
  const file = transcriptFor(windowId);
  return (file && turnFrom(file, expected)) ?? asScreen();
};

const NO_REPLY = "(no reply yet)";

const sameText = (a, b) => (a ?? "").replace(/\s+/g, " ").trim() === (b ?? "").replace(/\s+/g, " ").trim();

// Whether a transcript has caught up with a turn the Stop hook reported: its
// last text is the turn's final message.
const caughtUp = (file, final) => {
  const after = lastPrompt(file)?.after ?? [];
  const texts = after.flatMap((entry) =>
    entry.type === "assistant" && Array.isArray(entry.message?.content)
      ? entry.message.content.filter((part) => part.type === "text" && part.text.trim()).map((part) => part.text)
      : [],
  );
  return sameText(texts.at(-1), final);
};

// Whether a transcript's last turn has been written to the end: its last
// entry is Claude's text, with no tool call after it. Claude Code writes the
// last words a moment after the Stop hook fires, so a turn reported ended can
// still stop at a tool call, or at text that came before one.
const turnWritten = (file) => {
  const after = lastPrompt(file)?.after ?? [];
  const last = after.filter((entry) => entry.type === "assistant" || entry.type === "user").at(-1);
  const parts = Array.isArray(last?.message?.content) ? last.message.content : [];
  return last?.type === "assistant" && parts.some((part) => part.type === "text" && part.text.trim()) && !parts.some((part) => part.type === "tool_use");
};

// Whether a prompt is the text the relay typed: the same start, ignoring
// spacing, since Claude Code may store it reflowed.
// Claude Code keeps a long message it was typed (the relay types the whole
// of yours at once, as a paste) as pasted text, wrapped in a
// <pasted_content> tag; this is the text inside.
const unpasted = (text) => text.replace(/<pasted_content\b[^>]*>\n?([\s\S]*?)\n?<\/pasted_content\b[^>]*>/g, "$1");

// Whether a prompt holds the text the relay typed. Anywhere in it, not only
// at its start: text left unsent in Claude's input box (an earlier message
// whose Enter was lost, or your own typing) is sent along with it, in front.
const promptMatches = (prompt, expected) => {
  const words = (text) => unpasted(text).replace(/\s+/g, " ").trim();
  return words(prompt).includes(words(expected).slice(0, 30));
};

// A transcript's last prompt, when it was sent, and every entry after it, or
// undefined when there is no prompt yet. Subagents' entries are left out.
// Read from the end, a tail four times longer each time until it holds a
// prompt: a long session's transcript runs to hundreds of megabytes, and
// reading it all took two seconds, so the Home tab's Open, which reads it
// twice, outlived the three seconds Slack gives a button to open a screen.
const PROMPT_TAIL = 1024 * 1024;
const lastPrompt = (file, tail = PROMPT_TAIL) => {
  let lines;
  let whole;
  try {
    const size = fs.statSync(file).size;
    const length = Math.min(size, tail);
    whole = length === size;
    const buffer = Buffer.alloc(length);
    const fd = fs.openSync(file, "r");
    fs.readSync(fd, buffer, 0, length, size - length);
    fs.closeSync(fd);
    lines = buffer.toString("utf8");
  } catch {
    return undefined;
  }
  // A tail starts partway through a line.
  if (!whole) lines = lines.slice(lines.indexOf("\n") + 1);
  const entries = lines.split("\n").filter(Boolean).flatMap((line) => {
    try {
      const entry = JSON.parse(line);
      return entry.isSidechain || entry.isMeta ? [] : [entry];
    } catch {
      return [];
    }
  });
  const promptText = (entry) => {
    const content = entry.message?.content;
    if (typeof content === "string") return content;
    if (!Array.isArray(content) || content.some((part) => part.type === "tool_result")) return undefined;
    return content.filter((part) => part.type === "text").map((part) => part.text).join("\n") || undefined;
  };
  const start = entries.findLastIndex((entry) => entry.type === "user" && promptText(entry));
  if (start === -1) return whole ? undefined : lastPrompt(file, tail * 4);
  const raw = promptText(entries[start]);
  // When a background task finishes, Claude Code reports it to the session as
  // a prompt of its own (origin task-notification, promptSource system), a
  // <task-notification> block of ids, a file path and a summary. It reads as
  // what it is: the task's summary, and `background` says it wasn't you.
  const background = entries[start].origin?.kind === "task-notification" || /^\s*<task-notification>/.test(raw);
  const summaries = [...raw.matchAll(/<summary>([\s\S]*?)<\/summary>/g)].map((m) => m[1].trim());
  // A subagent's report back starts a turn the same way (origin peer), in
  // the harness's framing: it reads as the report, cut short. Known by its
  // origin alone, never its text, which a prompt of yours may quote.
  const origin = entries[start].origin;
  if (origin?.kind === "peer") {
    const follows = raw.indexOf("The report follows:");
    const framed = follows === -1 ? raw : raw.slice(follows + "The report follows:".length);
    const report = (origin?.handbackReportText ?? framed.replace(/<\/?agent-message[^>]*>/g, "")).replace(/\s+/g, " ").trim();
    const prompt = `An agent reported back: ${report.length > 200 ? `${report.slice(0, 200)}…` : report}`;
    return { prompt, background: true, sentAt: Date.parse(entries[start].timestamp ?? ""), after: entries.slice(start + 1) };
  }
  // Slash commands are stored with their tags; show what was typed.
  const prompt = background
    ? summaries.join("\n") || "A background task finished."
    : unpasted(raw)
        .replace(/<command-name>([^<]*)<\/command-name>/, "$1 ")
        .replace(/<[^>]+>[^<]*<\/[^>]+>/g, "")
        .trim();
  return { prompt, background, sentAt: Date.parse(entries[start].timestamp ?? ""), after: entries.slice(start + 1) };
};

// When a session last did something: the timestamp of its transcript's last
// conversation entry (a prompt, a reply, a tool call or result), in ms. Not
// the file's modified time: Claude Code also appends bookkeeping records
// (permission mode, PR link, …) to an idle session's transcript. Read from the
// last ACTIVITY_TAIL bytes, and again only when the file has changed.
const ACTIVITY_TAIL = 256 * 1024;
const activity = new Map();
const lastActivityAt = (file) => {
  let stat;
  try {
    stat = fs.statSync(file);
  } catch {
    return undefined;
  }
  const known = activity.get(file);
  if (known?.mtimeMs === stat.mtimeMs) return known.at;
  const length = Math.min(stat.size, ACTIVITY_TAIL);
  const buffer = Buffer.alloc(length);
  const fd = fs.openSync(file, "r");
  fs.readSync(fd, buffer, 0, length, stat.size - length);
  fs.closeSync(fd);
  const at = buffer
    .toString("utf8")
    .split("\n")
    .reverse()
    .reduce((found, line) => {
      if (found !== undefined || !/"type":"(user|assistant)"/.test(line)) return found;
      try {
        const entry = JSON.parse(line);
        const time = Date.parse(entry.timestamp ?? "");
        return (entry.type === "user" || entry.type === "assistant") && !Number.isNaN(time) ? time : undefined;
      } catch {
        return undefined;
      }
    }, undefined);
  activity.set(file, { mtimeMs: stat.mtimeMs, at });
  return at;
};

// Claude's last words in a transcript's last turn, or undefined.
const lastSaid = (file) =>
  (lastPrompt(file)?.after ?? [])
    .flatMap((entry) => (entry.type === "assistant" && Array.isArray(entry.message?.content) ? entry.message.content : []))
    .filter((part) => part.type === "text" && part.text.trim())
    .at(-1)?.text;

// The messages queued into a turn while it ran, in order (see turnFrom).
// Only what you typed: Claude Code queues a finished background task's
// notification and a subagent's report into the turn the same way, marked
// by their origin (task-notification, peer), and quoting those buried your
// message under ids, paths and the harness's framing.
const queuedIn = (after) =>
  after
    .filter((entry) => {
      const attachment = entry.attachment;
      const typed = (attachment?.origin?.kind ?? "human") === "human" && attachment?.commandMode !== "task-notification";
      return entry.type === "attachment" && attachment?.type === "queued_command" && typeof attachment.prompt === "string" && typed;
    })
    .map((entry) => entry.attachment.prompt);

// The last turn in a transcript file, or undefined when it has no prompt yet
// or, given `expected`, its last prompt is not that text.
// With `final` (the turn's last message, from the Stop hook), a transcript
// that hasn't caught up yet still ends with it.
const turnFrom = (file, expected, final) => {
  const last = lastPrompt(file);
  if (!last) return undefined;
  const { prompt, background, sentAt: sent, after } = last;
  // A Claude process can outlive its conversation (one that moved on to a
  // background session keeps the old transcript), so the transcript may not
  // be what the tab shows. When the relay knows what it typed, the last prompt
  // must be that text, or the screen is used instead. Otherwise the reply says
  // when the prompt was sent, so a stale turn is obvious.
  // A message sent while Claude is mid-turn joins that turn rather than
  // starting one: Claude Code records it as a queued_command attachment after
  // the prompt. The relay's text may be one of those.
  const queued = queuedIn(after);
  if (expected && !promptMatches(prompt, expected) && !queued.some((text) => promptMatches(text, expected))) return undefined;
  let tools = 0;
  const said = [];
  after.forEach((entry) => {
    if (entry.type !== "assistant") return;
    (entry.message?.content ?? []).forEach((part) => {
      if (part.type === "tool_use") tools++;
      if (part.type === "text" && part.text.trim()) said.push(part.text.trim());
    });
  });
  const quoted = [prompt, ...queued].join("\n").split("\n").map((line) => `> ${line}`).join("\n");
  if (final?.trim() && !sameText(said.at(-1), final)) said.push(final.trim());
  const reply = said.join("\n\n") || NO_REPLY;
  const ago = Number.isNaN(sent) ? "" : ` _(${background ? "a background task finished" : "prompt sent"} ${relative(Date.now() - sent)})_`;
  return `${ago.trim()}\n${quoted}\n\n${reply}${tools ? `\n\n_(ran ${tools} tool${tools === 1 ? "" : "s"})_` : ""}`.trim();
};

// Full lines, joined where tmux wrapped them, so nothing is cut short. A
// run of one character (Claude Code draws its rules as a full-width line of
// ─ or ╌, 270 of them on a wide window) is cut to a few: it says nothing,
// and Haiku can't copy hundreds of them exactly, so a post with a screen in it
// failed and went out as the fallback note.
const RULE_RUN = /([\u2500-\u257F═—_=~.·-])\1{5,}/g;
const screen = (windowId) => {
  const out = tryTmux(["capture-pane", "-p", "-J", "-t", paneFor(windowId), "-S", `-${SCREEN_LINES * 2}`]) ?? "";
  return out
    .split("\n")
    .map((line) => line.replace(RULE_RUN, "$1$1$1").trimEnd())
    .filter((line) => line.trim())
    .slice(-SCREEN_LINES)
    .join("\n");
};

// What a waiting tab is showing: a permission prompt is not in the transcript,
// only on the screen, and its numbered options are what you answer with.
const PROMPT_LINES = 20;
// Nothing when `turn`, what the message already shows, is the screen itself
// (lastTurn falls back to it).
// The numbered choices a waiting tab is showing (a permission prompt, or a
// question Claude asked), or undefined. Read from the bottom of the screen:
// the last run of lines numbered 1, 2, 3…, which may have description lines
// between them, and the few lines above it as the question.
//
// A question Claude asks with AskUserQuestion has more to it. Its options end
// with "Type something", for an answer of your own, and a rule sets "Chat about
// this" apart below; that last option above the rule is `free`, found by its
// place, since your text replaces its label. With several questions they come
// one at a time, under a line of their names marked ☐ or ☒, and answering one
// shows the next in the same wait. A question that takes several answers
// (`multi`) shows each option as [ ] or [✔] (`checked`): a number ticks or
// unticks it, and a Submit row after the options, reached with the arrow keys,
// moves on. `cursor` is the option the ❯ marks, or "submit".
const OPTION = /^\s*(?:[❯›>]\s*)?(\d{1,2})[.)]\s+(.*\S)\s*$/;
const CHECKBOX = /^\[([ ✔✓xX])\]\s+(.*)$/;
const SUBMIT_ROW = /^\s*(?:❯\s*)?Submit$/;
const RULE = /^[\s\u2500-\u257F—-]+$/;
// The names of the questions above a prompt of several.
const QUESTION_TABS = /[☐☒]/;
const optionsIn = (text) => {
  const lines = text.split("\n").map((line) => line.replace(/[│┃]/g, " ").trimEnd()).filter((line, i, all) => line.trim() && line !== all[i - 1]);
  const options = new Map();
  let first = -1;
  for (let i = lines.length - 1; i >= 0; i--) {
    const match = lines[i].match(OPTION);
    if (!match) {
      if (options.size && i < lines.length - 1 && !options.has(1)) continue;
      if (options.has(1)) break;
      continue;
    }
    const n = Number(match[1]);
    if (!options.has(n)) {
      const label = match[2].replace(/\s{2,}.*$/, "");
      const box = label.match(CHECKBOX);
      options.set(n, { n, label: box ? box[2] : label, ...(box ? { checked: box[1] !== " " } : {}), line: i, marked: /^\s*[❯›>]/.test(lines[i]) });
    }
    first = i;
    if (n === 1) break;
  }
  const numbers = [...options.keys()].sort((a, b) => a - b);
  if (numbers.length < 2 || numbers.some((n, i) => n !== i + 1)) return undefined;
  const found = numbers.map((n) => options.get(n));
  const last = found.at(-1).line;
  const rule = lines.findIndex((line, i) => i > first && i < last && RULE.test(line));
  const submit = lines.findIndex((line, i) => i > first && i < last && SUBMIT_ROW.test(line));
  const free = rule === -1 ? undefined : found.filter((o) => o.line < rule).at(-1)?.n;
  const multi = found.some((o) => o.checked !== undefined);
  const cursor = submit !== -1 && /❯/.test(lines[submit]) ? "submit" : found.find((o) => o.marked)?.n;
  // The question is the few lines just above the options, back to the
  // nearest rule (any line of box-drawing or dashes, such as the dotted one
  // under a file preview) or the names of the questions, so neither is taken
  // for the question. Under the names it is every line back to them, for the
  // review of your answers, one line per question and per answer.
  const above = [];
  const named = lines.slice(Math.max(0, first - 13), first).findLastIndex((line) => QUESTION_TABS.test(line)) !== -1;
  const most = named ? 12 : 3;
  for (let i = first - 1; i >= 0 && above.length < most && !RULE.test(lines[i]) && !QUESTION_TABS.test(lines[i]) && !OPTION.test(lines[i]); i--) above.unshift(lines[i].trim());
  // multi, free and cursor only for a question Claude asked, so a permission
  // prompt is just its question and options.
  return {
    question: above.join(named ? "\n" : " "),
    options: found.map(({ n, label, checked }) => ({ n, label, ...(checked === undefined ? {} : { checked }) })),
    ...(multi ? { multi } : {}),
    ...(free ? { free, cursor } : {}),
  };
};

// What a prompt shows, as Markdown: its question, its options (ticked or not,
// for one that takes several answers), and how to answer it from Slack.
const promptText = (found) => {
  const box = (o) => (o.checked === undefined ? "" : o.checked ? "☑ " : "☐ ");
  const hint = found.multi
    ? "Tap options to tick or untick them, then Submit choices. Or reply with their numbers, and `submit` when you're done."
    : "Tap an option, or reply with its number or the start of it.";
  const [heading, ...rest] = found.question.split("\n");
  return (
    `${heading ? `**${heading}**\n` : ""}${rest.map((line) => `${line}\n`).join("")}${found.options.map((o) => `${o.n}. ${box(o)}${o.label}`).join("\n")}` +
    `\n_${hint}${found.free ? ` Anything else you reply is typed in as option ${found.free}, your own answer.` : ""}_`
  );
};

// Your reply to a prompt, as what to press: an option's number ("choose"),
// options to tick or untick in one that takes several answers ("toggle", by
// one or more numbers, "1 3", or the start of one), Submit ("submit"), or your
// own answer in the free option ("free"). Undefined when it is none of those.
const answerFor = (found, text) => {
  const reply = text.trim();
  const fixed = { ...found, options: found.options.filter((o) => o.n !== found.free) };
  if (found.multi && /^(submit|done|next)$/i.test(reply)) return { kind: "submit" };
  if (found.multi && /^\d{1,2}(?:[\s,]+\d{1,2})+$/.test(reply)) {
    const options = [...new Set(reply.split(/[\s,]+/))].map((n) => chosenOption(fixed, n));
    if (options.every(Boolean)) return { kind: "toggle", options };
  }
  const option = chosenOption(fixed, reply);
  if (option) return found.multi ? { kind: "toggle", options: [option] } : { kind: "choose", option };
  if (found.free && reply) return { kind: "free", text: reply };
  return undefined;
};

// What an answer did, for the reply that says so.
const answerSaid = (answer) =>
  ({
    choose: () => `Chose ${answer.option.n}. ${answer.option.label}`,
    toggle: () => answer.options.map((o, i) => `${o.checked ? (i ? "unticked" : "Unticked") : i ? "ticked" : "Ticked"} ${o.n}. ${o.label}`).join(", "),
    free: () => `Answered "${answer.text}"`,
    submit: () => "Submitted your choices",
  })[answer.kind]();

// Presses what `answer` (from answerFor) says, in a pane showing `found`. A
// number picks or ticks its option. Submit and a free answer in a question
// that takes several are reached with the arrow keys from wherever the ❯ is,
// since a number only ticks; in one that takes one answer, the free option's
// number opens it for typing, and Enter sends what was typed.
const answerPrompt = (pane, found, answer) => {
  const rows = found.free ?? found.options.length;
  const at = found.cursor === "submit" ? rows + 1 : found.cursor ?? 1;
  const moveTo = (row) => Array.from({ length: Math.abs(row - at) }).forEach(() => tmux(["send-keys", "-t", pane, row > at ? "Down" : "Up"]));
  if (answer.kind === "submit") {
    moveTo(rows + 1);
    tmux(["send-keys", "-t", pane, "Enter"]);
  } else if (answer.kind === "free" && found.multi) {
    moveTo(found.free);
    pasteInto(pane, answer.text);
  } else if (answer.kind === "free") {
    tmux(["send-keys", "-t", pane, "-l", "--", String(found.free)]);
    pasteInto(pane, answer.text);
    tmux(["send-keys", "-t", pane, "Enter"]);
  } else {
    (answer.options ?? [answer.option]).forEach((o) => tmux(["send-keys", "-t", pane, "-l", "--", String(o.n)]));
  }
};

// The screen once it has changed from `before`, after an answer, or as it is
// after `waitMs` if it hasn't: what comes next (another question, or the
// turn going on) takes Claude Code a moment to draw.
const SCREEN_SETTLE_MS = 250;
const screenAfter = async (windowId, before, waitMs = 2_000) => {
  const until = Date.now() + waitMs;
  while (Date.now() < until && screen(windowId) === before) await new Promise((resolve) => setTimeout(resolve, SCREEN_SETTLE_MS));
  await new Promise((resolve) => setTimeout(resolve, SCREEN_SETTLE_MS));
  return screen(windowId);
};

// A short fingerprint of a prompt or dialog: its question and every option.
// A button records the one it was made for, so a press only ever answers that
// same prompt, not whatever the tab shows by the time it is pressed.
const promptHash = (found) =>
  crypto.createHash("sha256").update(`${found.question}\n${found.options.map((o) => `${o.n}. ${o.label}`).join("\n")}`).digest("hex").slice(0, 16);

// Your answer as one of the options: its number, or the start of its text.
const chosenOption = (found, text) => {
  if (!found) return undefined;
  if (/^\d{1,2}$/.test(text)) return found.options.find((o) => o.n === Number(text));
  return text.length >= 2 ? found.options.find((o) => o.label.toLowerCase().startsWith(text.toLowerCase())) : undefined;
};

// A dialog Claude Code shows before a session starts ("trust this folder",
// "new MCP server found"), as { question, options, cursor }, or undefined:
// options listed one per line, unnumbered, the current one marked ❯, above
// an "Enter to confirm" line at the bottom of the screen. No hook runs before
// the session starts, so a tab showing one has no state; the relay reads it
// from the screen. The options are the lines aligned with the text after ❯,
// so the question and notes above them, set less deep, aren't taken for one.
const dialogIn = (text) => {
  const lines = text.split("\n").filter((line, i, all) => line.trim() && line !== all[i - 1]);
  const footer = lines.findLastIndex((line) => /Enter to confirm/.test(line));
  if (footer === -1 || footer < lines.length - 2) return undefined;
  const cursorLine = lines.slice(0, footer).findLastIndex((line) => /❯\s+\S/.test(line));
  if (cursorLine === -1) return undefined;
  const mark = lines[cursorLine].indexOf("❯");
  const column = mark + 1 + lines[cursorLine].slice(mark + 1).search(/\S/);
  const isOption = (line) =>
    line.indexOf("❯") === mark || (/^\s*$/.test(line.slice(0, column)) && line.length > column && line[column] !== " ");
  let start = cursorLine;
  while (start > 0 && isOption(lines[start - 1])) start--;
  let end = cursorLine;
  while (end < footer - 1 && isOption(lines[end + 1])) end++;
  const options = lines.slice(start, end + 1).map((line, i) => ({ n: i + 1, label: line.replace("❯", "").trim() }));
  if (options.length < 2) return undefined;
  const rule = /^[\s\u2500-\u257F—-]+$/;
  const above = [];
  for (let i = start - 1; i >= 0 && above.length < 3 && !rule.test(lines[i]); i--) above.unshift(lines[i].trim());
  return { question: above.join(" "), options, cursor: cursorLine - start + 1 };
};

const onScreen = (windowId, turn) => {
  const found = optionsIn(screen(windowId));
  if (found) return `\n${promptText(found)}`;
  if (turn.startsWith("```")) return "";
  const lines = screen(windowId).split("\n").slice(-PROMPT_LINES).join("\n");
  return lines ? `\nOn screen now:\n\`\`\`\n${lines}\n\`\`\`` : "";
};

// Your message goes in as one paste, then Enter sends it. Pasted (through a
// buffer of the relay's own, so yours are untouched, with -p for bracketed
// paste when the program asks for it), its line breaks stay line breaks:
// typed with send-keys, each one reached Claude Code as a keypress, and a
// multi-line message arrived as one run-on line. Nothing in it is read as a
// key name either way.
// Enter waits PASTE_SETTLE_MS after the paste: sent at once, it can reach
// Claude Code before it has taken the paste in, and be lost, leaving the text
// unsent in its input box.
const PASTE_SETTLE_MS = 300;
const pasteInto = (pane, text) => {
  tmux(["set-buffer", "-b", "slack-relay", "--", text]);
  tmux(["paste-buffer", "-p", "-d", "-b", "slack-relay", "-t", pane]);
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, PASTE_SETTLE_MS);
};
const typeInto = (pane, text) => {
  pasteInto(pane, text);
  tmux(["send-keys", "-t", pane, "Enter"]);
};

module.exports = { CLAUDE_PANE, paneFor, PASTE_SETTLE_MS, unpasted, ACTIVITY_TAIL, lastActivityAt, queuedIn, lastSaid, promptHash, claudeTabs, label, STATE_WORDS, cell, table, claudePane, isAlive, transcriptFor, contextOf, contextWords, relative, lastTurn, NO_REPLY, sameText, caughtUp, turnWritten, promptMatches, lastPrompt, turnFrom, RULE_RUN, screen, PROMPT_LINES, OPTION, optionsIn, promptText, answerFor, answerSaid, answerPrompt, screenAfter, chosenOption, dialogIn, onScreen, pasteInto, typeInto };
