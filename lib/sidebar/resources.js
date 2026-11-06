// Part of bin/tmux-sidebar; see the comment at the top of that file.
//
// Mission control's Resources view: how much CPU and memory each tab uses,
// counting every process under its panes (Claude, a dev server, Neovim and
// its language servers, a test run), as a table heaviest first, to tell which
// tab is slowing the Mac down. macOS gives a process's CPU and memory only
// through ps, so the daemon runs one (refreshResources) every RESOURCES_MS
// while a mission control client has the view in front, and never otherwise.
// CPU is ps's %cpu, where 100% is one core; memory is resident, so memory
// two processes share counts in both.

const path = require("node:path");
const { execFile } = require("node:child_process");

const RESOURCES_MS = 3_000;
// A tab using this much or more needs you: a core pegged, or this many bytes.
const CPU_HEAVY = 100;
const MEMORY_HEAVY = 4 * 1024 ** 3;

// The processes as the last ps saw them, undefined before the first.
let processes;

const parsePs = (text) =>
  text
    .split("\n")
    .map((line) => /^\s*(\d+)\s+(\d+)\s+(\d+)\s+([\d.]+)\s+(.*)$/.exec(line))
    .filter(Boolean)
    .map(([, pid, ppid, rss, cpu, command]) => ({ pid: Number(pid), ppid: Number(ppid), memory: Number(rss) * 1024, cpu: Number(cpu), name: path.basename(command.trim().replace(/^-/, "")) }));

const refreshResources = () =>
  new Promise((resolve) =>
    execFile("ps", ["-A", "-o", "pid=,ppid=,rss=,%cpu=,comm="], { encoding: "utf8", timeout: 10_000, maxBuffer: 16 * 1024 * 1024 }, (err, stdout) => {
      if (!err) processes = parsePs(stdout);
      resolve();
    }),
  );

const bytes = (n) => (n >= 1024 ** 3 ? `${(n / 1024 ** 3).toFixed(1)} GB` : `${Math.round(n / 1024 ** 2)} MB`);

// Each tab's processes: its panes' and every one they started, as a tree
// from each pane's pid.
const tabUsage = (tabs, list) => {
  const children = new Map();
  list.forEach((p) => children.set(p.ppid, [...(children.get(p.ppid) ?? []), p]));
  const byPid = new Map(list.map((p) => [p.pid, p]));
  return tabs.map((tab) => {
    const seen = new Map();
    const walk = (p) => {
      if (!p || seen.has(p.pid)) return;
      seen.set(p.pid, p);
      (children.get(p.pid) ?? []).forEach(walk);
    };
    tab.panes.forEach((pane) => walk(byPid.get(Number(pane.pane_pid))));
    const own = [...seen.values()];
    const names = new Map();
    own.forEach((p) => {
      const group = names.get(p.name) ?? { name: p.name, count: 0, memory: 0 };
      group.count += 1;
      group.memory += p.memory;
      names.set(p.name, group);
    });
    return {
      tab,
      count: own.length,
      cpu: own.reduce((sum, p) => sum + p.cpu, 0),
      memory: own.reduce((sum, p) => sum + p.memory, 0),
      heaviest: [...names.values()].sort((a, b) => b.memory - a.memory).slice(0, 3),
    };
  });
};

// The view's columns, as Activity Monitor lays them out: the tab, then its
// figures lined up on the right.
const RESOURCE_COLUMNS = [
  { title: "Tab", align: "left" },
  { title: "% CPU", align: "right" },
  { title: "Memory", align: "right" },
  { title: "Processes", align: "right" },
];

// How heavy a tab is overall: its CPU and memory each as a share of what
// marks a tab as needing you, added, so a core pegged weighs as much as
// MEMORY_HEAVY of memory.
const weight = (u) => u.cpu / CPU_HEAVY + u.memory / MEMORY_HEAVY;

const figures = (u) => [u.cpu.toFixed(1), bytes(u.memory), String(u.count)];

// A row per tab, heaviest first; the chosen row adds its three heaviest kinds
// of process (`more`), which a / filter matches, as it does the figures.
const resourceItems = (tabs, list = processes) => {
  if (!list) return [];
  return tabUsage(tabs, list)
    .sort((a, b) => weight(b) - weight(a))
    .map((u) => {
      const heaviest = u.heaviest.map((g) => `${g.name}${g.count > 1 ? ` ×${g.count}` : ""} ${bytes(g.memory)}`).join(", ");
      return {
        title: `${u.tab.index} ${u.tab.name}`,
        detail: heaviest,
        cells: [`${u.tab.index} ${u.tab.name}`, ...figures(u)],
        more: heaviest,
        search: figures(u).join(" "),
        state: u.cpu >= CPU_HEAVY || u.memory >= MEMORY_HEAVY ? "attention" : u.cpu >= 1 ? "active" : "idle",
        tab: u.tab,
      };
    });
};

// The totals row under the table, for every tab together.
const resourceTotals = (tabs, list = processes) => {
  if (!list) return undefined;
  const usage = tabUsage(tabs, list);
  const sum = (key) => usage.reduce((total, u) => total + u[key], 0);
  return ["All tabs", ...figures({ cpu: sum("cpu"), memory: sum("memory"), count: sum("count") })];
};

const resourcesLoaded = () => processes !== undefined;

module.exports = { RESOURCES_MS, CPU_HEAVY, MEMORY_HEAVY, RESOURCE_COLUMNS, parsePs, refreshResources, resourceItems, resourceTotals, resourcesLoaded };
