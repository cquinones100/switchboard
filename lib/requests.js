// The request audit: every request Switchboard makes to a service outside
// this Mac, counted as it is made, for mission control's Usage view. Each is
// one line, "<epoch>\t<service>\t<protocol>\t<what made it>", appended to
// ~/.local/state/switchboard/requests/<day>.tsv, a file a day, kept
// KEEP_DAYS. The service is who answers (GitHub, Linear, Slack, Anthropic,
// ...); the protocol how it is reached: "GraphQL" and "REST" over HTTPS,
// "WebSocket", "iCal" over HTTPS, "MCP" (a Claude connector, reached through
// a headless claude -p, which is counted too, as the Anthropic API under
// "claude -p", once a run however many turns it takes), "git" and "HTTPS"
// for a plain page. Shell scripts count the same way with
// tmux/count-request.sh. Counting never fails what it counts.

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const KEEP_DAYS = 14;
const DIR = () => path.join(os.homedir(), ".local/state/switchboard/requests");
const ymd = (d) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
const fileFor = (d) => path.join(DIR(), `${ymd(d)}.tsv`);
const clean = (text) => String(text ?? "").replace(/[\t\n]/g, " ");

// A host on this Mac (a page served locally, read in the link view) is no
// service outside it, so it isn't counted.
const LOCAL = /^(localhost|127\.\d+\.\d+\.\d+|\[?::1\]?|0\.0\.0\.0)$/i;
const count = (service, protocol, source = "", n = 1) => {
  if (LOCAL.test(String(service))) return;
  try {
    // Once a day across every process that counts, Node's and the shell's:
    // .pruned holds the day it last ran.
    const marker = path.join(DIR(), ".pruned");
    const today = ymd(new Date());
    let last = "";
    try {
      last = fs.readFileSync(marker, "utf8").trim();
    } catch {}
    if (last !== today) {
      prune();
      fs.mkdirSync(DIR(), { recursive: true });
      fs.writeFileSync(marker, `${today}\n`);
    }
    fs.mkdirSync(DIR(), { recursive: true });
    const line = `${Math.floor(Date.now() / 1000)}\t${clean(service)}\t${clean(protocol)}\t${clean(source)}\n`;
    fs.appendFileSync(fileFor(new Date()), line.repeat(Math.max(1, n)));
  } catch {}
};

// A request made by running a command (gh), counted only when the command is
// there to run, since one that isn't makes no request. Where it is is looked
// up on the PATH and kept a minute.
const found = new Map();
const countRun = (command, service, protocol, source = "") => {
  const known = found.get(command);
  let there = known && Date.now() - known.at < 60_000 ? known.there : undefined;
  if (there === undefined) {
    there = (process.env.PATH ?? "").split(path.delimiter).some((dir) => {
      try {
        fs.accessSync(path.join(dir, command), fs.constants.X_OK);
        return true;
      } catch {
        return false;
      }
    });
    found.set(command, { there, at: Date.now() });
  }
  if (there) count(service, protocol, source);
};

// A day's counts: { total, services: [{ service, total, protocols: { protocol: n } }], protocols: { protocol: n } },
// the busiest service first.
const tally = (date = new Date()) => {
  let text = "";
  try {
    text = fs.readFileSync(fileFor(date), "utf8");
  } catch {}
  // With no prototype, so a service named after one of Object's own (a link's
  // host, "constructor") counts like any other.
  const services = Object.create(null);
  const protocols = Object.create(null);
  let total = 0;
  text.split("\n").forEach((line) => {
    const [, service, protocol] = line.split("\t");
    if (!service || !protocol) return;
    total += 1;
    services[service] ??= { service, total: 0, protocols: Object.create(null) };
    services[service].total += 1;
    services[service].protocols[protocol] = (services[service].protocols[protocol] ?? 0) + 1;
    protocols[protocol] = (protocols[protocol] ?? 0) + 1;
  });
  // Handed back as plain objects, the names their own keys.
  return { total, services: Object.values(services).sort((a, b) => b.total - a.total).map((s) => ({ ...s, protocols: { ...s.protocols } })), protocols: { ...protocols } };
};

// Days past KEEP_DAYS go.
const prune = (now = new Date()) => {
  const oldest = ymd(new Date(now.getTime() - KEEP_DAYS * 86400_000));
  try {
    fs.readdirSync(DIR())
      .filter((f) => /^\d{4}-\d{2}-\d{2}\.tsv$/.test(f) && f.slice(0, 10) < oldest)
      .forEach((f) => fs.rmSync(path.join(DIR(), f), { force: true }));
  } catch {}
};

module.exports = { KEEP_DAYS, count, countRun, tally, prune };
