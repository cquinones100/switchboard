// Part of bin/tmux-sidebar; see the comment at the top of that file.

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

// Kept short: a Unix socket's path can't be over 104 bytes on macOS.
const STATE_DIR = process.env.TMUX_SIDEBAR_STATE_DIR || path.join(os.homedir(), ".local/state/tmux-sidebar");
const SOCKET = path.join(STATE_DIR, "sock");
const LOCK = path.join(STATE_DIR, "daemon.pid");
const LOG_FILE = path.join(STATE_DIR, "daemon.log");
// Anything that changes what a tab shows appends a line here (agent-state.sh,
// window-names.sh, the tmux hooks), and the daemon redraws at once rather than
// at its next poll.
const NUDGE = path.join(STATE_DIR, "nudge");
// The tabs under Needs you, in the sidebar's order, one per line
// (<window>\t<pane or ->\t<session>), for M-a (tmux/agent-state.sh next) to walk.
const NEEDS = path.join(STATE_DIR, "needs");
// What M-A walks: the sidebar's Idle group, as the daemon writes it.
const IDLE = path.join(STATE_DIR, "idle");

// Yours, beside the rest of your Switchboard settings: which plugins load, and
// your own plugins.
const CONFIG_DIR = path.join(os.homedir(), ".config/switchboard");
const CONFIG_FILE = path.join(CONFIG_DIR, "sidebar.json");
const USER_PLUGINS = path.join(CONFIG_DIR, "sidebar-plugins");

// The zsh client every sidebar pane and the mission control popup run.
const CLIENT = process.env.TMUX_SIDEBAR_CLIENT || path.join(os.homedir(), ".config/tmux/sidebar-pane.zsh");
const THEME_FILE = path.join(os.homedir(), ".config/tmux/.theme-mode");
const CLAUDE_DIR = path.join(os.homedir(), ".claude");

// Sidebar panes are marked with this pane option and this title. The title is
// what tmux-resurrect saves and restores, so a sidebar that comes back from a
// restart as a plain shell is recognized and given its client again.
const MARK = "@sidebar";
const TITLE = "tmux-sidebar";

// Without a nudge the daemon still looks every POLL_MS, for what nothing
// announces: a tab closed in the background, a pane resized.
const POLL_MS = 5_000;
const SPINNER_MS = 120;

const DEFAULTS = {
  // Columns the sidebar takes, and the narrowest window that gets one.
  width: 34,
  minWindowWidth: 90,
  // Mission control cards.
  cardWidth: 56,
  cardHeight: 13,
  // plugin name -> its options; only plugins named here are loaded.
  plugins: {},
};

const readConfig = () => {
  try {
    return { ...DEFAULTS, ...JSON.parse(fs.readFileSync(CONFIG_FILE, "utf8")) };
  } catch {
    return { ...DEFAULTS };
  }
};

module.exports = { STATE_DIR, SOCKET, LOCK, LOG_FILE, NUDGE, NEEDS, IDLE, CONFIG_DIR, CONFIG_FILE, USER_PLUGINS, CLIENT, THEME_FILE, CLAUDE_DIR, MARK, TITLE, POLL_MS, SPINNER_MS, DEFAULTS, readConfig };
