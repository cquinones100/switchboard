// Part of bin/tmux-sidebar; see the comment at the top of that file.
//
// Plugins bring in things from outside tmux, from any service with an API: a
// plugin fetches a list of items, which mission control shows as a view of its
// own and the sidebar, if the plugin asks, as a short list under the tabs. A
// plugin is a CommonJS module:
//
//   module.exports = {
//     title: "Pull requests",       // the view's name
//     intervalSeconds: 300,         // how often fetch runs (default 300)
//     sidebar: 3,                   // items shown in the sidebar (default 0: none)
//     // The items, as of now. Runs in the daemon, never on the drawing path,
//     // so it may take its time and use the network.
//     async fetch({ options, run, keychain, log }) {
//       return [{ id, title, detail, state: "attention" | "active" | "idle", url, time }];
//     },
//     // What Enter does on an item; without it, its url opens in the browser.
//     async open(item, { options, run }) {},
//   };
//
// Only plugins named in ~/.config/switchboard/sidebar.json's "plugins" are
// loaded, each given what the config holds under its name as `options`. A
// name is looked for in ~/.config/switchboard/sidebar-plugins/<name>.js, which is
// yours, and then among those kept here in plugins/.

const { execFile } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");
const { promisify } = require("node:util");
const { USER_PLUGINS } = require("./config");

const execFileAsync = promisify(execFile);
const TIMEOUT_MS = 30_000;
const BUILT_IN = path.join(__dirname, "plugins");

// A command's output, for a plugin. Never through a shell, and given up on
// after TIMEOUT_MS.
const run = async (command, args = [], options = {}) =>
  (await execFileAsync(command, args, { encoding: "utf8", timeout: TIMEOUT_MS, maxBuffer: 16 * 1024 * 1024, ...options })).stdout;

// A secret from the macOS Keychain, so no token sits in the config file:
// `security add-generic-password -s <service> -a "$USER" -w` stores one.
const keychain = async (service) => (await run("security", ["find-generic-password", "-s", service, "-w"])).trim();

const loadPlugins = (config, log) =>
  Object.entries(config.plugins ?? {}).flatMap(([name, options]) => {
    if (!/^[a-z0-9][a-z0-9-]*$/.test(name) || options === false) return [];
    const file = [path.join(USER_PLUGINS, `${name}.js`), path.join(BUILT_IN, `${name}.js`)].find((f) => fs.existsSync(f));
    if (!file) {
      log(`plugin ${name}: no ${name}.js in ${USER_PLUGINS} or ${BUILT_IN}`);
      return [];
    }
    try {
      const plugin = require(file);
      return [{
        name,
        file,
        title: plugin.title ?? name,
        intervalSeconds: options?.intervalSeconds ?? plugin.intervalSeconds ?? 300,
        sidebar: options?.sidebar ?? plugin.sidebar ?? 0,
        plugin,
        options: options === true ? {} : options ?? {},
        items: [],
        loaded: false,
        error: undefined,
        fetching: undefined,
        fetchedAt: 0,
      }];
    } catch (err) {
      log(`plugin ${name}: ${err.message}`);
      return [];
    }
  });

// Runs a section's fetch, one at a time per plugin, keeping the last items
// when it fails. `changed` is called once the section has new items or an error.
const refreshSection = (section, changed, log) => {
  if (section.fetching) return section.fetching;
  section.fetching = (async () => {
    try {
      const items = await section.plugin.fetch({ options: section.options, run, keychain, log: (...parts) => log(`${section.name}:`, ...parts) });
      section.items = Array.isArray(items) ? items : [];
      section.error = undefined;
    } catch (err) {
      section.error = String(err.message ?? err).split("\n")[0];
      log(`plugin ${section.name}: ${section.error}`);
    } finally {
      section.loaded = true;
      section.fetchedAt = Date.now();
      section.fetching = undefined;
      changed();
    }
  })();
  return section.fetching;
};

// Starts every section's fetch now and on its interval.
const schedulePlugins = (sections, changed, log) =>
  sections.map((section) => {
    refreshSection(section, changed, log);
    return setInterval(() => refreshSection(section, changed, log), section.intervalSeconds * 1000).unref();
  });

const openItem = async (section, item, log) => {
  try {
    if (section.plugin.open) await section.plugin.open(item, { options: section.options, run });
    else if (item.url && /^https?:\/\//.test(item.url)) await run(path.join(__dirname, "../../tmux/open-url.sh"), [item.url]);
  } catch (err) {
    log(`plugin ${section.name}: open failed: ${err.message}`);
  }
};

module.exports = { loadPlugins, refreshSection, schedulePlugins, openItem, run, keychain };
