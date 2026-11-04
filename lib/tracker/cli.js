#!/usr/bin/env node
// The ticket tracker for the shell scripts in tmux/, through tmux/tracker.sh,
// which finds node for them. Each command asks whichever tracker the
// `tracker` setting names.
//
//   cli.js name           Linear or Jira; exits 2 with the setting off
//   cli.js config         where the skills file tickets, as JSON: { tracker,
//                         site, team (a Linear team or Jira project key),
//                         labels }, never a key or token
//   cli.js title <ID>     the ticket's title
//   cli.js show <ID>      the ticket as JSON, { data: { issue } }, for the
//                         ticket view (pr-view.js ticket)
//   cli.js project <ID>   the URL of its Linear project or Jira epic
//   cli.js url <ID>       the ticket's page, worked out without asking
//
// Exits 3 when the tracker has no such ticket, 4 when it is in no project,
// 2 with the setting off, and 1 with what went wrong on stderr.

const { tracker, idFrom } = require("./index");

const [command, arg] = process.argv.slice(2);
const provider = tracker();

const out = (text) => process.stdout.write(`${text}\n`);
const stop = (code, message) => {
  if (message) process.stderr.write(`${message}\n`);
  process.exit(code);
};

const main = async () => {
  if (!provider) stop(2, "The tracker setting is off.");
  if (command === "name") return out(provider.name);
  if (command === "config") return out(JSON.stringify({ tracker: provider.name, ...provider.filing() }));
  const id = idFrom(arg);
  if (!id) stop(1, `Not a ticket ID: ${arg ?? ""}`);
  switch (command) {
    case "title": {
      const found = await provider.issue(id);
      return found ? out(found.title) : stop(3);
    }
    case "show": {
      const found = await provider.detail(id);
      return found ? out(JSON.stringify({ data: { issue: found } })) : stop(3, `${provider.name} has no ticket ${id}.`);
    }
    case "project": {
      const found = await provider.project(id);
      if (found === undefined) stop(3, `${provider.name} has no ticket ${id}.`);
      return found?.url ? out(found.url) : stop(4);
    }
    case "url": {
      const url = provider.urlOf(id);
      return url ? out(url) : stop(1, provider.key === "jira" ? 'No Jira site set: add "jiraSite" to ~/.config/switchboard/jira.json.' : 'No Linear workspace set: add "linearWorkspace" to ~/.config/switchboard/linear.json.');
    }
    default:
      stop(1, "Usage: cli.js name | config | title|show|project|url <ID>");
  }
};

main().catch((err) => stop(1, String(err.message).split("\n")[0]));
