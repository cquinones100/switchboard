// Part of bin/tmux-sidebar; see the comment at the top of that file.

const { execFile } = require("node:child_process");
const { promisify } = require("node:util");
const { MARK } = require("./config");

const run = promisify(execFile);

// tmux rewrites control characters in its output, tab delimiters included,
// without a UTF-8 locale.
const ENV = { ...process.env, LC_ALL: "en_US.UTF-8" };

const tmux = async (args) => (await run("tmux", args, { encoding: "utf8", env: ENV, maxBuffer: 16 * 1024 * 1024 })).stdout.replace(/\n$/, "");
const tryTmux = async (args) => {
  try {
    return await tmux(args);
  } catch {
    return undefined;
  }
};

// Several commands in one tmux process: [["select-window", "-t", "@1"], ...].
const batch = (commands) => tryTmux(commands.flatMap((command, i) => (i ? [";", ...command] : command)));

// Text typed into Claude Code's prompt, Enter after it with `enter`. With
// Claude Code's vim mode, the prompt can sit in Normal mode (its mode line, at
// the screen's foot, reading -- NORMAL --), where the first letters typed are
// taken as vim commands until one starts Insert mode, so `i` goes first there.
// tmux/claude-pane.sh's claude_type does the same for the shell scripts.
const NORMAL_MODE = /-- NORMAL --/;
const typeIntoClaude = async (pane, text, { enter }) => {
  const screen = (await tryTmux(["capture-pane", "-p", "-t", pane])) ?? "";
  const foot = screen.split("\n").filter((line) => line.trim()).slice(-3).join("\n");
  await batch([
    ...(NORMAL_MODE.test(foot) ? [["send-keys", "-t", pane, "i"]] : []),
    ["send-keys", "-t", pane, "-l", text],
    ...(enter ? [["send-keys", "-t", pane, "Enter"]] : []),
  ]);
};

const HOLD_SECONDS = 120;

const PANE_FIELDS = [
  "session_id", "session_name", "session_attached",
  "window_id", "window_index", "window_name", "@tab_label", "@tab_name", "@tab_title", "window_active", "window_width", "window_zoomed_flag", "@agent", "@pinned", "@remind_at", "@remind_note", "@remind_said", "@pane_parked", "@notices_window", "@autopilot", "@autopilot_in", "@autopilot_item", "@autopilot_left", "@autopilot_seen", "@autopilot_land", "@autopilot_autoland", "window-status-format",
  "pane_id", "pane_active", "pane_current_path", "pane_current_command", "@agent_state", "@agent_since", "@agent_activity", "@agent_wait", "@agent_loop", "@agent_phase", "@parked_pane", "@notes_pane", "@notes_placeholder", MARK, "pane_title", "pane_width", "pane_height", "pane_start_command", "pane_left", "window-style", "pane_tty", "pane_pid",
];

// Everything the daemon needs from tmux, in one process: every pane, the
// clients, and the sidebar's own options. Undefined when there is no server.
const snapshot = async () => {
  const out = await tryTmux([
    "list-panes", "-a", "-F", PANE_FIELDS.map((f) => `#{${f}}`).join("\t"), ";",
    "list-clients", "-F", "C\t#{client_name}\t#{session_id}\t#{client_activity}\t#{client_control_mode}", ";",
    "display-message", "-p", "G\t#{@sidebar_on}\t#{@sidebar_hold}",
  ]);
  if (out === undefined) return undefined;
  const panes = [];
  const clients = [];
  let on = true;
  let hold = false;
  out.split("\n").forEach((line) => {
    const fields = line.split("\t");
    if (fields[0] === "C") {
      const [, name, session, activity, control] = fields;
      if (control !== "1") clients.push({ name, session, activity: Number(activity) });
    } else if (fields[0] === "G") {
      on = fields[1] !== "off";
      // Set to the time tmux-resurrect began restoring, and ignored after
      // HOLD_SECONDS in case a restore never finished.
      hold = Date.now() / 1000 - Number(fields[2]) < HOLD_SECONDS;
    } else if (fields.length === PANE_FIELDS.length) {
      const pane = Object.fromEntries(PANE_FIELDS.map((f, i) => [f, fields[i]]));
      panes.push(pane);
    }
  });
  return { panes, clients, on, hold };
};

// The client to act through for a session: the one used last.
const clientFor = (snap, session) =>
  snap.clients.filter((c) => c.session === session).sort((a, b) => b.activity - a.activity)[0]?.name ??
  snap.clients.sort((a, b) => b.activity - a.activity)[0]?.name;

module.exports = { tmux, tryTmux, batch, typeIntoClaude, snapshot, clientFor };
