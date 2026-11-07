#!/usr/bin/env node
// Cuts a piece out of a PNG, for looking at part of a tall diff image: Read
// can't show an image much taller than a screen.
//
//   node crop.js <png> --out <file> [--top <px>] [--height <px>] [--left <px>] [--width <px>]
//
// Pixels of the image, not CSS pixels: at 2x scale, double what the page
// measured. --top can be negative, counting from the bottom. The defaults are
// the top 1800 rows and the whole width, up to 2000.

const fs = require("node:fs");
const { PNG } = require("pngjs");

const args = process.argv.slice(2);
const flag = (name, fallback) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 ? Number(args[i + 1]) : fallback;
};
const out = args[args.indexOf("--out") + 1];
if (!args[0] || !args.includes("--out")) {
  console.error("usage: node crop.js <png> --out <file> [--top <px>] [--height <px>] [--left <px>] [--width <px>]");
  process.exit(2);
}
const src = PNG.sync.read(fs.readFileSync(args[0]));
const height = Math.min(flag("height", 1800), src.height);
const top = Math.max(0, Math.min(src.height - height, flag("top", 0) < 0 ? src.height + flag("top", 0) : flag("top", 0)));
const left = Math.max(0, Math.min(src.width - 1, flag("left", 0)));
const width = Math.min(flag("width", 2000), src.width - left);
const piece = new PNG({ width, height });
Array.from({ length: height }, (_, y) => y).forEach((y) => {
  const start = ((top + y) * src.width + left) * 4;
  src.data.copy(piece.data, y * width * 4, start, start + width * 4);
});
fs.writeFileSync(out, PNG.sync.write(piece));
console.log(`${out}: ${width}x${height} from ${src.width}x${src.height}, rows ${top} to ${top + height}`);
