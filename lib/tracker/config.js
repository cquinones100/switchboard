// Where lib/tracker finds what it needs to reach a tracker: the Keychain for
// secrets, and ~/.config/worktree-from-ticket/config.json, the file `wt` has
// always read, for the rest (Linear's workspace, Jira's site and email) and
// for a key kept there instead of the Keychain.

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { execFile } = require("node:child_process");

const CONFIG_FILE = path.join(process.env.XDG_CONFIG_HOME || path.join(os.homedir(), ".config"), "worktree-from-ticket", "config.json");

const configValue = (key) => {
  try {
    return JSON.parse(fs.readFileSync(CONFIG_FILE, "utf8"))[key] || undefined;
  } catch {
    return undefined;
  }
};

// A generic password from the macOS Keychain, or undefined.
const keychain = (service) =>
  new Promise((resolve) =>
    execFile("security", ["find-generic-password", "-s", service, "-w"], { encoding: "utf8", timeout: 5_000 }, (err, stdout) =>
      resolve(!err && stdout.trim() ? stdout.trim() : undefined),
    ),
  );

module.exports = { CONFIG_FILE, configValue, keychain };
