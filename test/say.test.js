// Tests for tmux/say.sh, which says something in a box in the middle of the
// screen in place of tmux's status-line message. Run with: node --test test/*.test.js
//
// A stand-in tmux keeps each call, so nothing reaches your own tmux.

const { test } = require("node:test");
const assert = require("node:assert/strict");
const { execFileSync, spawnSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const script = path.join(__dirname, "../tmux/say.sh");

test("say.sh opens a box sized to its text, the text in the environment, or a status-line message with message_boxes off", async () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "say-"));
  try {
    const calls = path.join(tmp, "calls");
    const setting = path.join(tmp, "setting");
    fs.writeFileSync(setting, "");
    // The stand-in answers the setting from a file, and keeps every call a line each.
    fs.writeFileSync(path.join(tmp, "tmux"), `#!/bin/sh\nprintf '%s\\n' "$*" >> '${calls}'\ncase "$1" in show) cat '${setting}' ;; esac\n`, { mode: 0o755 });
    const env = { ...process.env, PATH: `${tmp}:${process.env.PATH}`, XDG_STATE_HOME: tmp };
    const said = async () => {
      // The box opens in the background, so its call lands a moment later.
      for (let i = 0; i < 50 && !/display-|run-shell/.test(fs.existsSync(calls) ? fs.readFileSync(calls, "utf8") : ""); i++) await new Promise((resolve) => setTimeout(resolve, 50));
      const text = fs.readFileSync(calls, "utf8");
      fs.writeFileSync(calls, "");
      return text;
    };

    execFileSync("bash", [script, "-c", "client-1", "No #12 here"], { env });
    const box = await said();
    assert.match(box, /display-popup -c client-1 -E -w 30 -h 4 -b rounded -e SAY_TEXT=No #12 here -e SAY_SECONDS=5 .*say\.sh box/, "a box as wide as the text needs, at least 30");

    execFileSync("bash", [script, "-t", "2", "x".repeat(100)], { env });
    assert.match(await said(), /-w 78 -h 5 .*SAY_SECONDS=2/s, "a long line wrapped to 72 columns, and the box grown a row for it");

    fs.writeFileSync(setting, "off\n");
    execFileSync("bash", [script, "-c", "client-1", "No #12 here"], { env });
    assert.match(await said(), /display-message -c client-1 No ##12 here/, "off, the status line, its # doubled for tmux's format");

    fs.writeFileSync(setting, "");
    execFileSync("bash", [script, "-c", "client-1", "-a", "it's at #T"], { env });
    assert.match(await said(), /run-shell -b sleep 0\.4; .*say\.sh -c client-1 -t 5 it\\'s\\ at\\ ##T/, "-a: said by the server a moment later, after the popup it came from, its # doubled for run-shell's formats");

    // A box open on client-1 (its input held open, so it stays up) is closed for client-1 only.
    const open = require("node:child_process").spawn("bash", [script, "box", "client-1"], { env: { ...env, SAY_TEXT: "x", SAY_SECONDS: "30" }, stdio: ["pipe", "ignore", "ignore"] });
    try {
      await new Promise((resolve) => setTimeout(resolve, 200));
      execFileSync("bash", [script, "close", "client-2"], { env });
      assert.equal(fs.readFileSync(calls, "utf8"), "", "another client's popup is left alone");
      execFileSync("bash", [script, "close", "client-1"], { env });
      assert.match(await said(), /^display-popup -C -c client-1$/m);
    } finally {
      open.kill();
    }

    const shown = spawnSync("bash", [script, "box"], { env: { ...env, SAY_TEXT: "one\ntwo", SAY_SECONDS: "1" }, input: "q", encoding: "utf8" });
    assert.match(shown.stdout, /\n {2}one\n {2}two\n/, "the box shows the text, indented");
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
});
