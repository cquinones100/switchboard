// Switchboard's permissions in its panes, written into its plugin by
// `switchboard setup` (src/setup/permissions.rs, from
// src/setup/permissions.js). Claude Code reads permission rules from its
// settings files alone, and this plugin writes none, so this hook on
// Claude Code's own permission decision (tool.check) does two things, for
// the Claude sessions that load the plugin, the ones in Switchboard's panes:
//
// - ALLOW: the few of Switchboard's own commands its instructions have
//   Claude run on any turn (switchboard tabs, show, set, ...). Only one
//   simple command, with no operator, redirect, expansion or line break,
//   and only when Claude Code's own verdict is the plain ask it gives a
//   call no rule decided, so a deny or ask rule of yours, a hook's ask, a
//   check of its own (a read outside the working directories) or an
//   organization's limit still decides. The rest of Switchboard's rules are
//   its skills' own, in each skill's allowed-tools.
// - DENY: what Switchboard keeps Claude from doing in its panes, whatever
//   the verdict: reading a .env file (with the Read, Grep or Glob tools, or
//   a shell command that reads one) and git pull.
//
// Both lists are assets/claude-settings.json's rules, filled in below as
// the plugin is written. Settings' pane_permissions off leaves every call
// to Claude Code.

const ALLOW = [];
const DENY = [];

const SETTING = 'pane_permissions';

// One simple command's words, or null when it is anything else: an
// operator, a redirect, an expansion, a substitution, an escape, a glob, a
// comment, a line break. Single quotes are taken as written; double quotes
// may hold nothing the shell would expand.
export function words(command) {
  const out = [];
  let word = null;
  let quote = null;
  for (const c of String(command).trim()) {
    if (quote === "'") {
      if (c === "'") quote = null;
      else if (c === '\n') return null;
      else word += c;
    } else if (quote === '"') {
      if (c === '"') quote = null;
      else if ('$`\\!\n'.includes(c)) return null;
      else word += c;
    } else if (c === ' ' || c === '\t') {
      if (word !== null) out.push(word);
      word = null;
    } else if (c === "'" || c === '"') {
      quote = c;
      word = word ?? '';
    } else if (/[A-Za-z0-9_@%+=:,.\/-]/.test(c)) {
      word = (word ?? '') + c;
    } else {
      return null;
    }
  }
  if (quote !== null) return null;
  if (word !== null) out.push(word);
  return out;
}

// A rule `Bash(<command>)` as the words it starts with and what may follow
// them, as Claude Code reads the rule: nothing, one word or more (` *`), or
// the rest of the last word and anything after (`*`).
function bashRule(rule) {
  const m = /^Bash\((.+)\)$/.exec(rule);
  if (!m) return null;
  let text = m[1];
  let more = 'none';
  if (text.endsWith(' *')) {
    text = text.slice(0, -2);
    more = 'words';
  } else if (text.endsWith('*')) {
    text = text.slice(0, -1);
    more = 'prefix';
  }
  const start = words(text);
  return start && start.length > 0 ? { rule, start, more } : null;
}

function fits({ start, more }, w) {
  if (w.length < start.length) return false;
  const last = start.length - 1;
  for (let i = 0; i <= last; i++) {
    const ok = more === 'prefix' && i === last ? w[i].startsWith(start[i]) : w[i] === start[i];
    if (!ok) return false;
  }
  const rest = w.length - start.length;
  return more === 'none' ? rest === 0 : more === 'words' ? rest > 0 : true;
}

const ALLOWED = ALLOW.map(bashRule).filter(Boolean);

// Whether a shell command is one of the allowed commands, alone.
export function allows(command) {
  const w = words(command);
  return w !== null && ALLOWED.some((r) => fits(r, w));
}

const escape = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

// A rule `Read(<file>)` as a pattern on a path's last part: `.env`,
// `.env.*`, `**/.env` all name the file wherever it is, in any case, since
// a Mac's disk takes `.ENV` for `.env`.
function readRule(rule) {
  const m = /^Read\((?:\.\/|\*\*\/)?([^/]+)\)$/.exec(rule);
  if (!m) return null;
  const pattern = [...m[1]]
    .map((c) => (c === '*' ? '[^/]*' : c === '?' ? '[^/]' : escape(c)))
    .join('');
  return { rule, test: new RegExp(`^${pattern}$`, 'i') };
}

// A rule `Bash(<command>)` refused wherever the command starts one of a
// shell line's commands: after an operator, a path to the program, variables
// set before it, and options between the program and its subcommand (`git
// -C dir pull`) allowed.
function bashDenyRule(rule) {
  const r = bashRule(rule);
  if (!r) return null;
  const [program, ...rest] = r.start;
  let source = `(^|[;&|(\\n\`]|\\$\\()\\s*(\\w+=\\S*\\s+)*(\\S*/)?${escape(program)}`;
  source += '(\\s+-\\S+(\\s+[^-\\s;&|()]\\S*)?)*';
  for (const w of rest) source += `\\s+${escape(w)}`;
  if (r.more !== 'prefix') source += '(?=\\s|$|[;&|)`])';
  return { rule, test: new RegExp(source) };
}

const READS = DENY.map(readRule).filter(Boolean);
const RUNS = DENY.map(bashDenyRule).filter(Boolean);

// Programs whose file arguments are read and shown.
const READERS = new Set([
  'cat', 'tac', 'less', 'more', 'head', 'tail', 'nl', 'bat', 'batcat',
  'grep', 'egrep', 'fgrep', 'rg', 'ag', 'ack', 'sed', 'awk', 'gawk',
  'cut', 'sort', 'uniq', 'strings', 'xxd', 'od', 'hexdump', 'base64',
  'diff', 'cmp', 'jq', 'yq',
]);
// What may stand before a command's program.
const BEFORE = new Set(['sudo', 'env', 'command', 'exec', 'nice', 'time', 'xargs']);

const last = (path) => String(path).replace(/\/+$/, '').split('/').pop();
// Quotes taken out, as the shell takes them out: `.e""nv` is `.env`.
const unquote = (s) => String(s).replace(/['"]/g, '');

// The Read rule that names a path, if one does.
function readOf(path) {
  if (!path) return null;
  const name = last(unquote(String(path)));
  return READS.find((r) => r.test.test(name)) ?? null;
}

// The Read rule a shell command breaks by reading a file it names: one of
// the readers given the file, or a redirect from it.
function shellReadOf(command) {
  for (const m of command.matchAll(/<\s*([^\s;&|()<>]+)/g)) {
    const r = readOf(m[1]);
    if (r) return r;
  }
  for (const part of command.split(/&&|\|\||[;&|()\n`]|\$\(/)) {
    const tokens = part.trim().split(/\s+/).filter(Boolean);
    let i = 0;
    while (i < tokens.length && (/^\w+=/.test(tokens[i]) || BEFORE.has(tokens[i]))) i++;
    if (!READERS.has(last(unquote(tokens[i] ?? '')))) continue;
    for (const token of tokens.slice(i + 1)) {
      const value = unquote(token);
      const r = readOf(value.startsWith('-') && value.includes('=') ? value.slice(value.indexOf('=') + 1) : value);
      if (r) return r;
    }
  }
  return null;
}

// Why a call is refused, or null.
export function denied(tool, input) {
  let r = null;
  if (tool === 'Bash') {
    const command = String(input?.command ?? '');
    const bare = unquote(command);
    r = RUNS.find((d) => d.test.test(command) || d.test.test(bare)) ?? shellReadOf(command);
  } else if (tool === 'Read') {
    r = readOf(input?.file_path);
  } else if (tool === 'Grep') {
    r = readOf(input?.path) ?? readOf(input?.glob);
  } else if (tool === 'Glob') {
    r = readOf(input?.path) ?? readOf(input?.pattern);
  }
  return r ? `Switchboard's permissions in its panes deny this (${r.rule})` : null;
}

// Claude Code's verdict for a call no rule, hook, check or limit decided.
function plainAsk(v) {
  return v?.decision === 'ask' && !v.rule && !v.hook && !v.setting && !v.plugin && !v.ceiling;
}

// Settings' pane_permissions, as Switchboard reads it, asked once a load:
// off leaves every call to Claude Code.
let wanted;
async function askWanted($) {
  try {
    const { exitCode, stdout } = await $.process.run(['switchboard', 'setting', SETTING], {
      timeoutMs: 5000,
    });
    return !(exitCode === 0 && stdout.trim() === 'off');
  } catch {
    return true;
  }
}
function on($) {
  wanted ??= askWanted($);
  return wanted;
}

export function register(hook) {
  hook('tool.check', { tool: 'Bash' }, async ($, e, next) => {
    if (!(await on($))) return next(e);
    const no = denied('Bash', e.input);
    if (no) return { decision: 'deny', reason: no };
    const verdict = await next(e);
    if (plainAsk(verdict) && allows(e.input?.command ?? '')) {
      return { decision: 'allow', reason: "One of Switchboard's own commands, which its instructions run" };
    }
    return verdict;
  });
  for (const tool of ['Read', 'Grep', 'Glob']) {
    hook('tool.check', { tool }, async ($, e, next) => {
      if (!(await on($))) return next(e);
      const no = denied(tool, e.input);
      return no ? { decision: 'deny', reason: no } : next(e);
    });
  }
}
