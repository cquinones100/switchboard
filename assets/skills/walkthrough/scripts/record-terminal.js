#!/usr/bin/env node
// Records a walk through Switchboard's demo as a captioned video: the
// walkthrough skill's recorder for the terminal, beside record.js's for a
// browser, sharing its strips, MP4 and contact sheet (video.js).
//
//   node record-terminal.js <steps.js> --out <dir> [--name "<what it shows>"]
//        [--bin <switchboard>] [--size 160x45] [--pace demo|test]
//   node record-terminal.js --self-test
//
// It starts the demo (`switchboard demo`, made-up data in a home of its own
// that links nothing of yours) under a folder of its own in /tmp, so no
// demo of yours is touched, with --bin's build (a worktree passes its own
// target/release/switchboard to show its change; the switchboard on the PATH
// otherwise), attaches a client with no terminal to it (`switchboard
// headless`) and walks <steps.js>, which exports one async function given
// { step, keys, wait, expect }:
//
//   module.exports = async ({ step, keys, wait, expect }) => {
//     await step("Open mission control");
//     await keys("M-Space");
//     await expect("MISSION CONTROL");
//     await step("The next view of the group");
//     await keys("]", "j");
//     await wait(1500);
//   };
//
// step(text) captions what follows in a strip under the screen, and takes a
// frame of the screen as it is. keys(...) types each key into the whole
// workspace (`headless-keys`, by name: M-Space ] j Enter Escape), waits
// until the screen stops changing (two reads 100 ms apart that agree, at
// most 3 seconds) and takes a frame, shown with the keys of the call so far
// as keycaps in a strip over the screen, so the video says how each board is
// reached. wait(ms) holds the screen as it is that long, taking a frame.
// expect(text) checks the screen's text has it: at --pace demo a miss is
// noted in the log and the walk goes on, at --pace test (the default) it
// stops the run.
//
// Each frame is held for how long its step reads: at --pace demo at least
// 2.5 s for a step's own frame and 0.8 s for each key's, at --pace test 1.2 s
// and 0.4 s; a wait's frame for its time. Once done it stops the client and
// ends and removes its demo.
//
// Writes <dir>/<name>.mp4 (named as record.js names its own), <dir>/contact.png,
// <dir>/end.png (the last frame) and <dir>/log.txt, and prints
// { passed, video, contact, screenshot, log, error, timeline } as JSON, the
// timeline being each frame's second, caption and keys.
//
// --self-test checks what needs no server: the timeline a made-up walk makes,
// and, with Playwright's packages and Chrome, a video made from made-up
// frames. It prints { timeline, video } ("ok", or why a part was skipped) and
// exits 1 when a part fails.

const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const video = require("./video.js");

const args = process.argv.slice(2);
const arg = (name) => (args.includes(`--${name}`) ? args[args.indexOf(`--${name}`) + 1] : undefined);

// How long each kind of frame is held, in seconds, by pace.
const HOLD = { demo: { step: 2.5, key: 0.8 }, test: { step: 1.2, key: 0.4 } };

// The walk itself, given `shoot` (takes a frame into a file), `press` (types
// one key), `screen` (the screen's text) and `settle` (waits until the screen
// stops changing): the frames it took, each { file, hold, caption, keys }, and
// the expects that failed, kept in `made` as they come, so a walk that stops
// part way still has the frames up to there.
const walk = async (steps, { pace, frames: dir, shoot, press, screen, settle, log, made = { frames: [], failures: [] } }) => {
  const { frames, failures } = made;
  let caption = "";
  const take = async (hold, keys) => {
    const file = path.join(dir, `${String(frames.length).padStart(4, "0")}.png`);
    await shoot(file);
    frames.push({ file, hold, caption, keys });
  };
  const step = async (text) => {
    log.push(`step: ${text}`);
    caption = text;
    await take(HOLD[pace].step, []);
  };
  const keys = async (...names) => {
    for (const [i, name] of names.entries()) {
      log.push(`keys: ${name}`);
      await press(name);
      await settle();
      await take(HOLD[pace].key, names.slice(0, i + 1));
    }
  };
  const wait = async (ms) => take(Math.max(ms / 1000, HOLD[pace].key), []);
  const expect = async (text) => {
    if ((await screen()).includes(text)) return;
    const message = `expected the screen to show ${JSON.stringify(text)}`;
    log.push(`expect failed: ${message}`);
    if (pace === "test") throw new Error(message);
    failures.push(message);
  };
  await steps({ step, keys, wait, expect });
  return made;
};

// Each frame's second in the video, and the strips over and under it: a
// caption from a step's frame to the next step's, the keys while their frame
// shows.
const timelineOf = (frames) => {
  let at = 0;
  const timed = frames.map((frame) => {
    const item = { ...frame, at };
    at += frame.hold;
    return item;
  });
  const captions = timed.filter((frame, i) => i === 0 || frame.caption !== timed[i - 1].caption).map((frame) => ({ at: frame.at, text: frame.caption }));
  const items = [
    ...video.spans(captions).filter((item) => item.text).map((item) => ({ kind: "caption", ...item })),
    ...timed.filter((frame) => frame.keys.length).map((frame) => ({ kind: "keys", keys: frame.keys, at: frame.at, until: frame.at + frame.hold })),
  ];
  return { frames: timed, items, length: at };
};

// The video of the frames, each held its time, with the strips round it.
const render = async (frames, out, name, log) => {
  const { frames: timed, items } = timelineOf(frames);
  const dir = fs.mkdtempSync(path.join(out, "frame-"));
  try {
    // ffmpeg's concat list: each picture and how long it shows, the last
    // named again so its time counts.
    const list = path.join(dir, "frames.txt");
    const quote = (file) => `'${file.replace(/'/g, "'\\''")}'`;
    fs.writeFileSync(list, `${timed.map((frame) => `file ${quote(frame.file)}\nduration ${frame.hold.toFixed(3)}\n`).join("")}file ${quote(timed.at(-1).file)}\n`);
    const { width, height } = video.dimensions(timed[0].file);
    // The frames are drawn at two pixels to a CSS pixel (headless-screen's
    // SCALE), and so are the strips, to match.
    const scale = 2;
    const { layers, cut } = await video.strips(dir, items, { width: width / scale, scale });
    cut.forEach((text) => log.push(`caption cut short, too long for two lines: ${text}`));
    const mp4 = path.join(out, `${name}.mp4`);
    return video.compose({ input: ["-f", "concat", "-safe", "0", "-i", list], width, height, top: video.KEYS * scale, bottom: video.CAPTION * scale, rate: 25, layers, mp4 });
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
};

// The demo's environment, as `demo --print` prints it (export K=V lines, V
// quoted for a shell as src/worktree.rs's shell_quote does), alone, as the
// prefix `demo --detached` prints runs it (env -i).
const parseExports = (text) => {
  const env = {};
  for (const line of text.split("\n")) {
    const match = /^export ([A-Za-z_][A-Za-z0-9_]*)=(.*)$/.exec(line);
    if (!match) continue;
    const value = match[2];
    env[match[1]] = value.startsWith("'") ? value.slice(1, -1).replace(/'\\''/g, "'") : value;
  }
  return env;
};

const record = async () => {
  const stepsFile = args[0];
  const out = arg("out") && path.resolve(arg("out"));
  const pace = arg("pace") ?? "test";
  const size = arg("size") ?? "160x45";
  if (!stepsFile || stepsFile.startsWith("--") || !out || !HOLD[pace] || !/^\d+x\d+$/.test(size)) {
    console.error('usage: node record-terminal.js <steps.js> --out <dir> [--name "<what it shows>"] [--bin <switchboard>] [--size <cols>x<rows>] [--pace demo|test]');
    process.exit(2);
  }
  video.needFfmpeg();
  fs.mkdirSync(out, { recursive: true });
  const bin = arg("bin") ? path.resolve(arg("bin")) : "switchboard";
  const frames = fs.mkdtempSync(path.join(out, "frames-"));
  const log = [];
  // A demo of its own: under a root no other demo uses (short, since the
  // server's socket path can't be over 104 bytes on macOS), ended and removed
  // once done.
  const root = fs.mkdtempSync("/tmp/sbw-");
  const outside = { ...process.env, SWITCHBOARD_DEMO_ROOT: root };
  let env = null;
  let client = null;
  const run = (argv) => execFileSync(path.join(env.HOME, ".local/bin/switchboard"), argv, { encoding: "utf8", env, stdio: ["ignore", "pipe", "pipe"] });
  const result = { passed: true, error: null };
  const made = { frames: [], failures: [] };
  try {
    env = parseExports(execFileSync(bin, ["demo", "--print"], { encoding: "utf8", env: outside, stdio: ["ignore", "pipe", "pipe"] }));
    if (!env.HOME) throw new Error("switchboard demo --print named no home");
    client = run(["headless", "--size", size]).trim();
    const screen = async () => run(["headless-screen", client]);
    const settle = async () => {
      const deadline = Date.now() + 3000;
      let before = run(["headless-screen", client, "--ansi"]);
      while (Date.now() < deadline) {
        await new Promise((resolve) => setTimeout(resolve, 100));
        const now = run(["headless-screen", client, "--ansi"]);
        if (now === before) return;
        before = now;
      }
    };
    await settle();
    await walk(require(path.resolve(stepsFile)), {
      made,
      pace,
      frames,
      log,
      shoot: async (file) => run(["headless-screen", client, "--png", file]),
      press: async (key) => run(["headless-keys", client, key]),
      screen,
      settle,
    });
  } catch (error) {
    result.passed = false;
    result.error = (error.stderr?.toString().trim() || error.message).split("\n")[0];
    log.push(`failed: ${error.stderr?.toString().trim() || error.message}`);
  } finally {
    if (client) {
      try {
        run(["headless", "--stop", client]);
      } catch {}
    }
    try {
      execFileSync(bin, ["demo", "--clean"], { env: outside, stdio: "ignore" });
    } catch {}
    fs.rmSync(root, { recursive: true, force: true });
  }
  if (made.failures.length) {
    result.passed = false;
    result.error ??= made.failures[0];
  }
  if (!made.frames.length && result.passed) {
    result.passed = false;
    result.error = "the steps took no frames, so there is no video: start them with step()";
  }
  const name = video.videoName(arg("name"));
  let recorded = null;
  let screenshot = null;
  if (made.frames.length) {
    screenshot = path.join(out, "end.png");
    fs.copyFileSync(made.frames.at(-1).file, screenshot);
    try {
      recorded = await render(made.frames, out, name, log);
    } catch (error) {
      // A run with no video failed, whatever its steps did, and keeps its
      // frames to look at.
      const why = `no video: ${error.message.split("\n")[0]}`;
      log.push(`${why}; the frames are kept in ${frames}`);
      result.passed = false;
      result.error ??= why;
    }
  }
  if (recorded || !made.frames.length) fs.rmSync(frames, { recursive: true, force: true });
  const contact = recorded && video.contactSheet(recorded, out);
  const logFile = video.writeLog(out, log);
  const timeline = timelineOf(made.frames).frames.map(({ at, caption, keys }) => ({ at: Math.round(at * 1000) / 1000, caption, keys }));
  console.log(JSON.stringify({ ...result, video: recorded, contact, screenshot, log: logFile, timeline }, null, 2));
};

// What needs no server: a walk of two steps and three keys makes five frames,
// held their times, the keys on their frames; and three made-up frames make a
// video as tall as a frame and both strips.
const selfTest = async () => {
  const report = {};
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "walkthrough-self-test-"));
  try {
    const shot = [];
    const { frames, failures } = await walk(
      async ({ step, keys, expect }) => {
        await step("Open mission control");
        await keys("M-Space");
        await expect("MISSION CONTROL");
        await step("The next view");
        await keys("]", "j");
        await expect("not there");
      },
      { pace: "demo", frames: dir, log: [], shoot: async (file) => shot.push(file), press: async () => {}, settle: async () => {}, screen: async () => "MISSION CONTROL" },
    );
    const { frames: timed, items } = timelineOf(frames);
    const want = [
      { at: 0, hold: 2.5, caption: "Open mission control", keys: [] },
      { at: 2.5, hold: 0.8, caption: "Open mission control", keys: ["M-Space"] },
      { at: 3.3, hold: 2.5, caption: "The next view", keys: [] },
      { at: 5.8, hold: 0.8, caption: "The next view", keys: ["]"] },
      { at: 6.6, hold: 0.8, caption: "The next view", keys: ["]", "j"] },
    ];
    const got = timed.map(({ at, hold, caption, keys }) => ({ at: Math.round(at * 10) / 10, hold, caption, keys }));
    const captions = items.filter((item) => item.kind === "caption").map(({ at, text }) => [Math.round(at * 10) / 10, text]);
    const ok =
      shot.length === 5 &&
      JSON.stringify(got) === JSON.stringify(want) &&
      JSON.stringify(captions) === JSON.stringify([[0, "Open mission control"], [3.3, "The next view"]]) &&
      items.filter((item) => item.kind === "keys").length === 3 &&
      failures.length === 1;
    report.timeline = ok ? "ok" : `wrong: ${JSON.stringify({ shot: shot.length, got, captions, failures })}`;

    let playwright = true;
    try {
      require.resolve("playwright");
    } catch {
      playwright = false;
    }
    if (!playwright) report.video = "skipped: Playwright's packages aren't installed (npm ci in this folder)";
    else {
      try {
        video.needFfmpeg();
        const made = ["red", "green", "blue"].map((color, i) => {
          const file = path.join(dir, `${i}.png`);
          execFileSync("ffmpeg", ["-y", "-loglevel", "error", "-f", "lavfi", "-i", `color=c=${color}:s=640x360`, "-frames:v", "1", file]);
          return { file, hold: 0.8, caption: "Three frames", keys: i ? ["j"] : [] };
        });
        const mp4 = await render(made, dir, "self-test", []);
        const { width, height } = video.dimensions(mp4);
        const tall = 360 + 2 * (video.KEYS + video.CAPTION);
        report.video = width === 640 && height === tall ? "ok" : `wrong: ${width}x${height}, not 640x${tall}`;
      } catch (error) {
        report.video = /chrome|executable/i.test(error.message) ? `skipped: no Chrome (${error.message.split("\n")[0]})` : `wrong: ${error.message.split("\n")[0]}`;
      }
    }
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
  console.log(JSON.stringify(report));
  if (Object.values(report).some((value) => value.startsWith("wrong"))) process.exit(1);
};

(args.includes("--self-test") ? selfTest() : record()).catch((error) => {
  console.error(error.message);
  process.exit(1);
});
