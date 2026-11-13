// The video both of the walkthrough's recorders make, so one caption strip and
// one contact sheet serve a browser (record.js) and Switchboard's demo
// (record-terminal.js): the strips laid round the recording, each taking
// space of its own rather than covering it (the browser's address bar above
// it, or the keys just pressed, and the caption under it), the MP4, and a
// sheet of frames to look it over at once. Taken out of record.js.
//
// The strips are drawn as images by the system Chrome through Playwright,
// one for each address, each caption and each run of keys, and laid round the
// recording by ffmpeg (a strip padded above and below), each shown from when
// it began to when the next did.

const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");

// Each strip's height, in CSS pixels.
const BAR = 52;
const KEYS = 52;
const CAPTION = 76;

// The system ffmpeg lays the strips round the video and makes the MP4, so a
// recording without it would show none of them: asked for before anything is
// recorded.
const needFfmpeg = () => {
  try {
    execFileSync("ffmpeg", ["-version"], { stdio: "ignore" });
  } catch {
    throw new Error("ffmpeg is needed to make the video, with its captions: install it (brew install ffmpeg) and run again");
  }
};

// The video's name: --name, else the Switchboard tab this runs in, as the tab
// bar names it (@tab_label, asked of the server with `switchboard show`, as it
// was of tmux), made a file name the Slack relay finds in a message
// (files.js's MEDIA_PATH takes ASCII letters, digits, . _ - and no spaces):
// anything else dropped, each run of spaces a dash, and cut to 80
// characters; "walkthrough" outside Switchboard or for a tab with no name.
const fileName = (text) => text.replace(/[^A-Za-z0-9 ._-]/g, "").trim().replace(/\s+/g, "-").replace(/-{2,}/g, "-").slice(0, 80).replace(/^[.-]+|[.-]+$/g, "");
const videoName = (name) => {
  if (name && fileName(name)) return fileName(name);
  if (!process.env.SWITCHBOARD_PANE) return "walkthrough";
  try {
    const label = execFileSync("switchboard", ["show", "-t", process.env.SWITCHBOARD_PANE, "@tab_label"], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] });
    return fileName(label) || "walkthrough";
  } catch {
    return "walkthrough";
  }
};

// Each change shown from when it came to when the next did.
const spans = (list) => list.map((item, i) => ({ ...item, until: list[i + 1]?.at ?? 1e6 })).filter((item) => item.until > item.at);

// Draws each strip of `items` ({ kind: "bar" | "keys" | "caption", text or
// keys, at, until }) as a PNG in `dir`, `width` CSS pixels wide at `scale`
// pixels to each, as the recording is. Returns the strips as layers ({ file,
// kind, at, until }) and the captions cut short, too long for two lines.
const strips = async (dir, items, { width, scale }) => {
  const { chromium } = require("playwright");
  const browser = await chromium.launch({ channel: "chrome", headless: true });
  try {
    const page = await browser.newPage({ viewport: { width, height: CAPTION }, deviceScaleFactor: scale });
    const layers = [];
    const cut = [];
    for (const [i, item] of items.entries()) {
      const height = { bar: BAR, keys: KEYS, caption: CAPTION }[item.kind];
      await page.setViewportSize({ width, height });
      await page.setContent(`<!doctype html><style>
        body { margin: 0; font: 15px -apple-system, system-ui, sans-serif; }
        .bar { height: ${BAR}px; box-sizing: border-box; padding: 0 14px; background: #dee1e6; display: flex; align-items: center; gap: 14px; color: #5f6368; }
        .nav { font-size: 18px; letter-spacing: 10px; }
        .pill { flex: 1; height: 34px; border-radius: 17px; background: #fff; display: flex; align-items: center; gap: 8px; padding: 0 14px; overflow: hidden; white-space: nowrap; }
        .address { overflow: hidden; text-overflow: ellipsis; } .host { color: #202124; } .rest { color: #5f6368; }
        .keys { height: ${KEYS}px; box-sizing: border-box; padding: 0 24px; background: #1e1e2a; display: flex; align-items: center; gap: 10px; overflow: hidden; white-space: nowrap; }
        kbd { font: 600 17px ui-monospace, Menlo, monospace; color: #9a9ab0; background: #2c2c3c; border: 1px solid #45455a; border-bottom-width: 3px; border-radius: 6px; padding: 3px 10px; }
        kbd.now { color: #14141e; background: #f2cc60; border-color: #b8952f; }
        .caption { height: ${CAPTION}px; box-sizing: border-box; padding: 0 24px; background: #14141e; color: #fff; display: flex; align-items: center; font: 600 21px -apple-system, system-ui, sans-serif; }
        .caption span { display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; }
      </style>
      <div class="bar" id="bar"><span class="nav">←→↻</span><div class="pill"><svg id="lock" width="12" height="14" viewBox="0 0 12 14"><rect x="1" y="6" width="10" height="7" rx="1.5" fill="#5f6368"/><path d="M3.5 6V4a2.5 2.5 0 0 1 5 0v2" stroke="#5f6368" stroke-width="1.6" fill="none"/></svg><span class="address"><span class="host" id="host"></span><span class="rest" id="rest"></span></span></div></div>
      <div class="keys" id="keys"></div>
      <div class="caption" id="caption"><span id="text"></span></div>`);
      // The text set as text, never as markup: a page's address, a key or a
      // caption is data. A caption too long for two lines is drawn smaller,
      // down to 15px; one that still doesn't fit is cut short, and said so
      // (false).
      const fits = await page.evaluate(([kind, value]) => {
        for (const id of ["bar", "keys", "caption"]) if (id !== kind) document.getElementById(id).remove();
        if (kind === "bar") {
          let parsed = null;
          try {
            parsed = new URL(value);
          } catch {}
          // The lock only for https, as a browser shows it.
          if (parsed?.protocol !== "https:") document.getElementById("lock").remove();
          document.getElementById("host").textContent = parsed ? parsed.host : value;
          document.getElementById("rest").textContent = parsed ? parsed.pathname + parsed.search + parsed.hash : "";
          return true;
        }
        if (kind === "keys") {
          // The keys pressed so far in this keys() call, the last one lit.
          const strip = document.getElementById("keys");
          value.forEach((key, i) => {
            const cap = document.createElement("kbd");
            cap.textContent = key;
            if (i === value.length - 1) cap.className = "now";
            strip.appendChild(cap);
          });
          return true;
        }
        const span = document.getElementById("text");
        span.textContent = value;
        for (let px = 21; px >= 15; px -= 1) {
          span.style.fontSize = `${px}px`;
          if (span.scrollHeight <= span.clientHeight + 1) return true;
        }
        return false;
      }, [item.kind, item.kind === "keys" ? item.keys : item.text]);
      const file = path.join(dir, `${item.kind}-${i}.png`);
      await page.screenshot({ path: file });
      if (!fits) cut.push(item.text);
      layers.push({ file, kind: item.kind, at: item.at, until: item.until });
    }
    return { layers, cut };
  } finally {
    await browser.close().catch(() => {});
  }
};

// A recording `width` by `height` pixels scaled down to at most `maxWidth`
// wide, keeping its aspect, each side even, as H.264 needs; as it is with no
// `maxWidth` or when it is narrower.
const fit = ({ width, height }, maxWidth) => {
  if (!maxWidth || width <= maxWidth) return { width, height };
  const even = (n) => Math.max(2, 2 * Math.round(n / 2));
  return { width: even(maxWidth), height: even((height * maxWidth) / width) };
};

// The MP4: the recording (ffmpeg's `input` arguments, `width` by `height`
// pixels, at `rate` frames a second when given, for pictures each held a
// while), scaled down to `maxWidth` when given and wider (`fit`), padded by
// `top` pixels above and `bottom` below, each layer drawn in its strip while
// it lasts: an address bar or keys at the top, a caption at the bottom. The
// layers are drawn at the scaled width.
const compose = ({ input, width: drawnWidth, height: drawnHeight, maxWidth, top, bottom, rate, layers, mp4 }) => {
  const { width, height } = fit({ width: drawnWidth, height: drawnHeight }, maxWidth);
  const steady = rate ? `fps=${rate},` : "";
  const scaled = width === drawnWidth ? "" : `scale=${width}:${height}:flags=lanczos,`;
  const filters = [`[0:v]${steady}${scaled}pad=${width}:${height + top + bottom}:0:${top}:color=0x14141e[v0]`];
  layers.forEach((layer, i) => {
    const y = layer.kind === "caption" ? top + height : 0;
    filters.push(`[v${i}][${i + 1}:v]overlay=0:${y}:enable='between(t,${layer.at.toFixed(3)},${layer.until.toFixed(3)})'[v${i + 1}]`);
  });
  execFileSync("ffmpeg", [
    "-y", "-loglevel", "error", ...input,
    ...layers.flatMap((layer) => ["-i", layer.file]),
    "-filter_complex", filters.join(";"), "-map", `[v${layers.length}]`,
    "-c:v", "libx264", "-pix_fmt", "yuv420p", "-movflags", "+faststart", mp4,
  ]);
  return mp4;
};

// A frame every two seconds, five across, for the first minute: the video
// looked over at once. Nothing when it couldn't be made.
const contactSheet = (video, out) => {
  try {
    const sheet = path.join(out, "contact.png");
    execFileSync("ffmpeg", ["-y", "-loglevel", "error", "-i", video, "-vf", "fps=1/2,scale=320:-1,tile=5x6:padding=6:color=white", "-frames:v", "1", sheet]);
    return sheet;
  } catch {
    return null;
  }
};

// A picture's or a video's width and height in pixels, as ffmpeg reads its
// first video stream (`ffmpeg -i` with no output, which says so and exits 1),
// so nothing but ffmpeg is needed.
const dimensions = (file) => {
  let said = "";
  try {
    execFileSync("ffmpeg", ["-hide_banner", "-i", file], { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
  } catch (error) {
    said = error.stderr?.toString() ?? "";
  }
  const match = /Video:.*?\b(\d{2,})x(\d{2,})\b/.exec(said);
  if (!match) throw new Error(`ffmpeg couldn't read the size of ${file}`);
  return { width: Number(match[1]), height: Number(match[2]) };
};

// The log, a line an entry, as log.txt in `out`.
const writeLog = (out, log) => {
  const file = path.join(out, "log.txt");
  fs.writeFileSync(file, `${log.join("\n")}\n`);
  return file;
};

module.exports = { BAR, KEYS, CAPTION, needFfmpeg, fileName, videoName, spans, strips, fit, compose, contactSheet, dimensions, writeLog };
