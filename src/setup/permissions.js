// Switchboard's permissions in its panes, written into its plugin by
// `switchboard setup` (src/setup/permissions.rs, from
// src/setup/permissions.js). Claude Code reads permission rules from its
// settings files alone, and this plugin writes none, so this hook on
// Claude Code's own permission decision (tool.check) does two things, for
// the Claude sessions that load the plugin, the ones in Switchboard's panes:
//
// - ALLOW: the few of Switchboard's own commands its instructions have
//   Claude run on any turn (switchboard tabs, show, set, ...), and the
//   landing's own, switchboard land-lock open-pr and item-done, which
//   queue-and-clean runs on a turn that read its skill rather than started
//   it, each only with the options it gives them (SHAPES). Only one
//   simple command, with no operator, redirect, expansion or line break,
//   and only when Claude Code's own verdict is the plain ask it gives a
//   call no rule decided, so a deny or ask rule of yours, a hook's ask, a
//   check of its own (a read outside the working directories) or an
//   organization's limit still decides. The rest of Switchboard's rules are
//   its skills' own, in each skill's allowed-tools.
// - DENY: what Switchboard keeps Claude from doing in its panes, whatever
//   the verdict: reading a .env file (with the Read, Grep or Glob tools, or
//   a shell command that reads one), git pull, and the options of the
//   git commands Switchboard allows that write a file, run a program or
//   fetch with a program or from an address of the command's choosing
//   (git log --output, git diff --ext-diff, git fetch --upload-pack,
//   git fetch --multiple); and what the repository's way to land
//   (`switchboard landing`) forbids: `gh pr merge` where GitHub doesn't
//   merge its PRs, and a push to its default branch where it doesn't land
//   straight on it, naming the way and Settings' row that sets it.
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
// may hold nothing the shell would expand; outside quotes a backslash may
// stand only before a single quote, as `'it'\''s'` writes an apostrophe.
export function words(command) {
  const out = [];
  let word = null;
  let quote = null;
  let escaped = false;
  for (const c of String(command).trim()) {
    if (escaped) {
      if (c !== "'") return null;
      word = (word ?? '') + c;
      escaped = false;
    } else if (quote === "'") {
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
    } else if (c === '\\') {
      escaped = true;
    } else if (/[A-Za-z0-9_@%+=:,.\/-]/.test(c)) {
      word = (word ?? '') + c;
    } else {
      return null;
    }
  }
  if (quote !== null || escaped) return null;
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

// The landing's commands as queue-and-clean gives them: the options each
// takes, those that take a value and those that don't, and nothing else.
// item-done's worktree as a word of its own and --checked-in, whose branch
// it takes on trust, are left to Claude Code, so the hook never lets it
// remove another worktree or one whose work is on no remote.
const SHAPES = [
  {
    start: ['switchboard', 'item-done'],
    values: ['--says', '--hash', '--already-on', '--merged-pr-head'],
    switches: ['--without-asking', '--close-issue'],
  },
  {
    start: ['switchboard', 'land-lock', 'open-pr'],
    values: ['--title', '--body-file'],
    switches: [],
  },
];

function asGiven(w) {
  const shape = SHAPES.find(({ start }) => start.every((s, i) => w[i] === s));
  if (!shape) return true;
  for (let i = shape.start.length; i < w.length; i++) {
    if (shape.switches.includes(w[i])) continue;
    if (!shape.values.includes(w[i]) || i + 1 >= w.length) return false;
    i++;
  }
  return true;
}

// Whether a shell command is one of the allowed commands, alone.
export function allows(command) {
  const w = words(command);
  return w !== null && asGiven(w) && ALLOWED.some((r) => fits(r, w));
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
// set before it, a wrapper running it (`env git pull`, `command git pull`),
// and options between the program and its subcommand (`git -C dir pull`)
// allowed. A rule `Bash(<command> *<option>*)` refuses the command when the
// rest of it, up to the next operator, holds the option anywhere (`git log
// -p --output=x`, `git log 2>&1 --output=x` once masked below), erring
// toward refusing rather than reading the words between; the option a
// prefix, since git takes a long option's unambiguous start for it
// (`git fetch --upl=x`).
function bashDenyRule(rule) {
  const option = /^Bash\((.+) \*(-[^\s*]+)\*\)$/.exec(rule);
  const r = bashRule(option ? `Bash(${option[1]} *)` : rule);
  if (!r) return null;
  const [program, ...rest] = r.start;
  const wrapper = `(\\S*/)?(${[...BEFORE].join('|')})(\\s+-\\S+)*`;
  const redirect = '\\d*(<|>>?)&?\\S*';
  const start = '(^|[;&|(\\n`]|\\$\\()';
  let source = `${start}\\s*((\\w+=\\S*|${wrapper}|${redirect})\\s+)*(\\S*/)?${escape(program)}`;
  source += '(\\s+-\\S+(\\s+[^-\\s;&|()]\\S*)?)*';
  for (const w of rest) source += `\\s+${escape(w)}`;
  if (option) {
    // The subcommand as a word of its own: `git diff`, not `git diff-tree`.
    source += '(?=\\s|$|[;&|)`])';
    const command = new RegExp(source);
    const each = new RegExp(source, 'g');
    // The rest of the command, up to the next operator, holds the option.
    const holds = (s) =>
      [...s.matchAll(each)].some((m) => s.slice(m.index + m[0].length).split(/[;&|\n]/)[0].includes(option[2]));
    // Whether the rest of the command, read where the command's text has
    // it, expands something; a command found only once masked, anywhere.
    const unread = (s) => {
      const found = [...s.matchAll(each)].filter((m) => !quotedAt(s, m.index + m[0].length - 1));
      const count = (t) => [...t.matchAll(each)].length;
      const read = [count(masked(s)), count(plain(s))];
      // Commands only the masks find, or a different count: the whole line.
      // Commands only the masks find, or a different count: each command of
      // the line the masks find it in, read on its own.
      if (read.some((n) => n !== found.length)) {
        return commands(s).some(([from, to]) => {
          const one = s.slice(from, to);
          return (command.test(masked(one)) || command.test(plain(one))) && expands(s, from, true, from);
        });
      }
      if (found.length === 0) return false;
      return found.some((m) => {
        // From the command's start, its global options (`git -c $X log`) too.
        const from = m.index + Math.max(0, m[0].search(/[^\s;&|(!{`$]/));
        return expands(s, from, true, m.index + m[0].length);
      });
    };
    return { rule, test: { test: holds }, command, unread };
  }
  if (r.more !== 'prefix') source += '(?=\\s|$|[;&|)`])';
  return { rule, test: new RegExp(source), command: null, unread: null };
}

// What may stand before a command's program.
const BEFORE = new Set(['sudo', 'env', 'command', 'exec', 'nice', 'time', 'xargs']);

const READS = DENY.map(readRule).filter(Boolean);
const RUNS = DENY.map(bashDenyRule).filter(Boolean);

// Readers whose first argument that isn't an option is a pattern or a
// program (`grep .env README.md`), not a file, unless one was given by an
// option; an `-e`/`--regexp`/`--expression` option's value is one too.
const PATTERN_FIRST = new Set([
  'grep', 'egrep', 'fgrep', 'rg', 'ag', 'ack', 'sed', 'awk', 'gawk', 'jq', 'yq',
]);
const PATTERN_OPTIONS = new Set(['-e', '--regexp', '--expression']);
const PROGRAM_FILE_OPTIONS = new Set(['-f', '--file', '--from-file']);

// Programs whose file arguments are read and shown.
const READERS = new Set([
  'cat', 'tac', 'less', 'more', 'head', 'tail', 'nl', 'bat', 'batcat',
  'grep', 'egrep', 'fgrep', 'rg', 'ag', 'ack', 'sed', 'awk', 'gawk',
  'cut', 'sort', 'uniq', 'strings', 'xxd', 'od', 'hexdump', 'base64',
  'diff', 'cmp', 'jq', 'yq',
]);

const last = (path) => String(path).replace(/\/+$/, '').split('/').pop();
// Quotes and escapes taken out, as the shell takes them out: `.e""nv` and
// `.e\nv` are `.env`.
const unquote = (s) => String(s).replace(/['"\\]/g, '');

// The command with what quotes or a backslash keep from the shell made
// part of the word it is in: a quoted or escaped operator or space becomes
// `_`, so `git log --format='a;b' --output=x` is one command of three
// words, as the shell runs it. Read in one pass, as the shell reads quotes
// and escapes together: a backslash escapes outside quotes, and inside
// double quotes only before `"`, `\`, `$`, a backtick and a line break; a
// backslash and a line break go, joining the lines, as the shell takes them.
function masked(s) {
  const kept = (c) => (/[;&|()`\s]/.test(c) ? '_' : c);
  let out = '';
  let quote = null;
  const chars = [...String(s)];
  for (let i = 0; i < chars.length; i++) {
    const c = chars[i];
    if (quote === "'") {
      if (c === "'") quote = null;
      else out += kept(c);
    } else if (c === '\\' && i + 1 < chars.length && (quote === null || '"\\$`\n'.includes(chars[i + 1]))) {
      const next = chars[++i];
      if (next !== '\n') out += kept(next);
    } else if (quote === '"') {
      if (c === '"') quote = null;
      else out += kept(c);
    } else if (c === "'" || c === '"') {
      quote = c;
    } else {
      out += c;
    }
  }
  // A redirect's ampersand (`2>&1`, `&>file`) runs no command after it.
  return out.replace(/([<>])&/g, '$1_').replace(/&>/g, '_>');
}

// Whether a command expands something before it runs (`$VAR`, `$(...)`,
// `$'\x41'`, a backtick) outside single quotes, or a brace list or range
// (`--out{put,}`), a glob (`--out*`, which a file of that name answers) or a
// tilde starting a word (`~`, the home a variable before it set) outside any
// quotes, so its words can't be read from its text. A brace with
// no comma or `..` in it (`@{u}`) is only a brace. From `start`, up to the
// next operator outside quotes, a redirect's ampersand not one, unless
// `stop` is false: then the whole line. An operator before `until`, inside
// the command as matched (`X=a;b git log`), doesn't end it.
function expands(s, start = 0, stop = true, until = start) {
  let quote = null;
  // Read from the line's start, so the quotes before `start` are known, but
  // only what is at `start` or after counts.
  const chars = String(s).split('');
  for (let i = 0; i < chars.length; i++) {
    const c = chars[i];
    if (quote === "'") {
      if (c === "'") quote = null;
    } else if (c === '\\') {
      i++;
    } else if (i < start) {
      if (c === '"') quote = quote === '"' ? null : '"';
      else if (c === "'" && quote === null) quote = "'";
    } else if (stop && i >= until && quote === null && ';|\n'.includes(c)) {
      return false;
    } else if (
      stop &&
      i >= until &&
      quote === null &&
      c === '-' &&
      chars[i + 1] === '-' &&
      /\s/.test(chars[i - 1] ?? ' ') &&
      /^[\s;&|)]?$/.test(chars[i + 2] ?? '')
    ) {
      // After `--` every word is a path, which no expansion makes an option.
      return false;
    } else if (stop && i >= until && quote === null && c === '&' && chars[i - 1] !== '>' && chars[i - 1] !== '<' && chars[i + 1] !== '>') {
      return false;
    } else if (c === '$' || c === '`') {
      return true;
    } else if (quote === null && '*?['.includes(c)) {
      return true;
    } else if (quote === null && c === '~' && (i === 0 || /[\s=:]/.test(chars[i - 1]))) {
      return true;
    } else if (quote === null && c === '{') {
      const close = chars.indexOf('}', i);
      const inside = chars.slice(i + 1, close < 0 ? chars.length : close).join('');
      if (inside.includes(',') || inside.includes('..')) return true;
    } else if (c === '"') {
      quote = quote === '"' ? null : '"';
    } else if (c === "'" && quote === null) {
      quote = "'";
    }
  }
  return false;
}

// Where each command of a line starts and ends: the spans between operators
// outside quotes (`;`, `|`, `&` but a redirect's, a line break).
function commands(s) {
  const spans = [];
  let quote = null;
  let from = 0;
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (quote === "'") {
      if (c === "'") quote = null;
    } else if (c === '\\') {
      i++;
    } else if (c === '"') {
      quote = quote === '"' ? null : '"';
    } else if (c === "'" && quote === null) {
      quote = "'";
    } else if (quote === null && (';|\n'.includes(c) || (c === '&' && s[i - 1] !== '>' && s[i - 1] !== '<' && s[i + 1] !== '>'))) {
      spans.push([from, i]);
      from = i + 1;
    }
  }
  spans.push([from, s.length]);
  return spans;
}

// Whether the character at `at` is inside quotes, as the shell reads them.
function quotedAt(s, at) {
  let quote = null;
  for (let i = 0; i < at; i++) {
    const c = s[i];
    if (quote === "'") {
      if (c === "'") quote = null;
    } else if (c === '\\') {
      i++;
    } else if (c === '"') {
      quote = quote === '"' ? null : '"';
    } else if (c === "'" && quote === null) {
      quote = "'";
    }
  }
  return quote !== null;
}

// A masked command with its redirects taken out, target and all, wherever
// they stand (`2> /dev/null git log`, `git 2>/dev/null log`), as the shell
// takes them out of the words it runs.
const plain = (s) => masked(s).replace(/_?\d*(<<?|>>?)_?\s*[^\s;&|()<>]*/g, ' ');

// A command with its quoted heredocs' bodies taken out, the data they are,
// which the shell neither runs, expands nor reads quotes in: the lines after
// a line with `<<'WORD'` (or `<<"WORD"`, `<<-`) outside quotes, through the
// line that is the word. An unquoted heredoc's body stays: the shell runs
// its substitutions (`$(cat .env)`).
function withoutHeredocs(s) {
  const out = [];
  const waiting = [];
  for (const line of String(s).split('\n')) {
    if (waiting.length > 0) {
      const [word, tabs] = waiting[0];
      if ((tabs ? line.replace(/^\t+/, '') : line) === word) waiting.shift();
      continue;
    }
    out.push(line);
    // Outside the line's comment, which the shell doesn't read for one.
    const code = uncommented(line);
    for (const m of code.matchAll(/(?<!<)<<(?!<)(-?)\s*(['"])([A-Za-z0-9_]+)\2/g)) {
      if (!quotedAt(code, m.index)) waiting.push([m[3], m[1] === '-']);
    }
  }
  // A heredoc whose word never comes isn't one to trust: the command as given.
  return waiting.length > 0 ? String(s) : out.join('\n');
}

// A command with its comments taken out: a `#` outside quotes that starts a
// word, to the end of its line, which the shell never runs or reads quotes
// in.
function uncommented(s) {
  let out = '';
  let quote = null;
  // Whether the next character starts a word: after unquoted, unescaped
  // space or an operator.
  let start = true;
  const chars = [...String(s)];
  for (let i = 0; i < chars.length; i++) {
    const c = chars[i];
    if (quote === null && c === '#' && start) {
      while (i + 1 < chars.length && chars[i + 1] !== '\n') i++;
      continue;
    }
    out += c;
    if (quote === "'") {
      if (c === "'") quote = null;
    } else if (c === '\\' && i + 1 < chars.length) {
      // A backslash and a line break go, leaving the word where it was.
      if (chars[i + 1] !== '\n') start = false;
      out += chars[++i];
    } else if (c === '"') {
      quote = quote === '"' ? null : '"';
      start = false;
    } else if (c === "'" && quote === null) {
      quote = "'";
      start = false;
    } else if (quote === null) {
      start = /[\s;&|()]/.test(c);
    }
  }
  return out;
}

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
    const reader = last(unquote(tokens[i] ?? ''));
    if (!READERS.has(reader)) continue;
    const args = tokens.slice(i + 1).map(unquote);
    const patternGiven = args.some(
      (a) => PATTERN_OPTIONS.has(a.split('=')[0]) || PROGRAM_FILE_OPTIONS.has(a.split('=')[0]),
    );
    let patternLeft = PATTERN_FIRST.has(reader) && !patternGiven;
    for (let k = 0; k < args.length; k++) {
      const value = args[k];
      if (PATTERN_FIRST.has(reader) && PATTERN_OPTIONS.has(value)) {
        k++;
        continue;
      }
      if (PATTERN_FIRST.has(reader) && PATTERN_OPTIONS.has(value.split('=')[0])) continue;
      if (patternLeft && !value.startsWith('-')) {
        patternLeft = false;
        continue;
      }
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
    const original = String(input?.command ?? '');
    const bodyless = withoutHeredocs(original);
    const command = uncommented(bodyless);
    const forms = [command, unquote(command), masked(command), plain(command)];
    // The rules that aren't an option's read the command as given too, its
    // comments and heredocs kept, so taking them out only ever spares a git
    // command, never what they deny (`$(cat .env)` in a heredoc's body).
    const kept = [original, unquote(original)];
    // An option rule reads only the forms that know quotes, so a quoted
    // message naming the command (`git commit -m '...git diff --output'`)
    // is no command.
    const known = forms.slice(2);
    r =
      RUNS.find((d) => (d.command ? known : [...forms, ...kept]).some((f) => d.test.test(f))) ??
      shellReadOf(command) ??
      shellReadOf(original);
    // A command an option is denied for, given words that are only known
    // once the shell expands them, could hold the option.
    const unread = !r && RUNS.find((d) => d.unread?.(command));
    if (unread) {
      return `Switchboard's permissions in its panes deny this (${unread.rule}): its words hold an expansion ($, a backtick, a brace list or a glob), so the option can't be ruled out; run it with the values written out`;
    }
  } else if (tool === 'Read') {
    r = readOf(input?.file_path);
  } else if (tool === 'Grep') {
    r = readOf(input?.path) ?? readOf(input?.glob);
  } else if (tool === 'Glob') {
    r = readOf(input?.path) ?? readOf(input?.pattern);
  }
  return r ? `Switchboard's permissions in its panes deny this (${r.rule})` : null;
}

// What a repository's way to land forbids: `gh pr merge` where the way
// doesn't merge PRs through GitHub, and a push to the default branch where
// the way isn't straight on it (`switchboard landing`'s `lands`, src/repos.rs's
// `Way`). A stacked PR, whose base isn't the default branch, is merged by
// GitHub's auto-merge whatever the way but a merge service, as
// queue-and-clean's "GitHub's auto-merge" says. A repository with no way set,
// or a push whose branch can't be read from its words (`git push origin
// "$b"`), is left to the rules above: this keeps a Claude from landing
// another way by mistake, and reads the directory a `-C` names, else the
// session's, never one a `cd` before it moves to.
const MERGES = new Set(['auto-merge', 'github-queue']);
const WAYS = {
  'switchboard-queue': "Switchboard's merge queue",
  'github-queue': "GitHub's merge queue",
  'auto-merge': "GitHub's auto-merge",
  direct: 'Straight to the default branch',
  service: 'A merge service',
};

// Options of git's and of `git push`'s and `gh pr merge`'s that take the
// next word as their value.
const GIT_VALUES = new Set(['-C', '-c', '--git-dir', '--work-tree', '--namespace', '--exec-path', '--config-env']);
const PUSH_VALUES = new Set(['--repo', '-o', '--push-option', '--receive-pack', '--exec']);
const PUSH_ALL = new Set(['--all', '--mirror', '--branches']);
const MERGE_VALUES = new Set([
  '-R', '--repo', '-b', '--body', '-F', '--body-file', '-t', '--subject', '--match-head-commit',
  '-A', '--author-email',
]);

// Each `git push` and `gh pr merge` a command runs, as what has to be looked
// up to judge it: a push's directory (`-C`) and the branches it pushes to
// (null for a bare push, which goes where `@{push}` says; `*` for every
// branch; `HEAD` for the current one), a merge's arguments. Read from each
// command of the line with its quotes taken out and its redirects dropped.
export function landingCalls(command) {
  const text = uncommented(withoutHeredocs(String(command)));
  const found = [];
  for (const [from, to] of commands(text)) {
    const w = plain(text.slice(from, to)).trim().split(/\s+/).filter(Boolean);
    let i = 0;
    while (i < w.length && (/^\w+=/.test(w[i]) || BEFORE.has(last(w[i])))) i++;
    const program = last(w[i] ?? '');
    if (program === 'git') {
      let dir = null;
      i++;
      while (i < w.length && w[i].startsWith('-')) {
        if (w[i] === '-C') dir = w[i + 1] ?? null;
        i += GIT_VALUES.has(w[i]) ? 2 : 1;
      }
      if (w[i] !== 'push') continue;
      const positional = [];
      let all = false;
      for (i++; i < w.length; i++) {
        if (w[i] === '--') {
          positional.push(...w.slice(i + 1));
          break;
        }
        if (PUSH_ALL.has(w[i])) all = true;
        else if (PUSH_VALUES.has(w[i])) i++;
        else if (!w[i].startsWith('-')) positional.push(w[i]);
      }
      const branches = positional.slice(1).map((r) => {
        const spec = r.replace(/^\+/, '');
        const dest = spec.includes(':') ? spec.slice(spec.indexOf(':') + 1) : spec;
        return dest.replace(/^refs\/heads\//, '');
      });
      found.push({ kind: 'push', dir, to: all ? ['*'] : branches.length > 0 ? branches : null });
    } else if (program === 'gh' && w[i + 1] === 'pr' && w[i + 2] === 'merge') {
      const args = [];
      for (let k = i + 3; k < w.length; k++) {
        if (MERGE_VALUES.has(w[k])) {
          if (w[k] === '-R' || w[k] === '--repo') args.push(w[k], w[k + 1] ?? '');
          k++;
        } else if (/^--repo=/.test(w[k]) || !w[k].startsWith('-')) {
          args.push(w[k]);
        }
      }
      found.push({ kind: 'merge', args });
    }
  }
  return found;
}

// Why the repository's way refuses a command, or null. `look` answers what
// the command's words can't: `landing(dir)` (`switchboard landing --json`,
// null when it fails, as it does with no way set), `branch(dir, ref)` (the
// branch `HEAD` or `@{push}` names, null when none) and `prBase(args)` (the
// PR's base branch, null when it can't be read).
export async function landingRefused(command, look) {
  for (const call of landingCalls(command)) {
    const landing = await look.landing(call.kind === 'push' ? call.dir : null);
    const way = landing?.lands;
    const main = landing?.defaultBranch;
    if (!way || !main) continue;
    const name = `${landing.repo ?? 'This repository'} lands by ${WAYS[way] ?? way} (\`${way}\`)`;
    const change = "land it as /queue-and-clean does, or choose another way on the repository's row in Settings' Repositories";
    if (call.kind === 'push' && way !== 'direct') {
      let to = call.to ?? [await look.branch(call.dir, '@{push}')];
      to = await Promise.all(to.map((t) => (t === 'HEAD' ? look.branch(call.dir, 'HEAD') : t)));
      if (to.some((t) => t === '*' || t === main)) {
        return `Switchboard's permissions in its panes deny this: ${name}, not a push to its default branch, ${main}: ${change}`;
      }
    } else if (call.kind === 'merge' && !MERGES.has(way)) {
      const base = way === 'service' ? main : await look.prBase(call.args);
      if (base === null || base === main) {
        return `Switchboard's permissions in its panes deny this: ${name}, not \`gh pr merge\`: ${change}`;
      }
    }
  }
  return null;
}

// The lookups `landingRefused` asks for, on the host, each only for a
// command that pushes or merges.
function lookOn($) {
  const run = async (argv, cwd) => {
    try {
      const init = cwd ? { cwd, timeoutMs: 15000 } : { timeoutMs: 15000 };
      const { exitCode, stdout } = await $.process.run(argv, init);
      return exitCode === 0 ? stdout.trim() : null;
    } catch {
      return null;
    }
  };
  return {
    landing: async (dir) => {
      const out = await run(['switchboard', 'landing', '--json'], dir);
      try {
        return out ? JSON.parse(out) : null;
      } catch {
        return null;
      }
    },
    branch: async (dir, ref) => {
      const out = await run(['git', 'rev-parse', '--abbrev-ref', ref], dir);
      if (!out || out === 'HEAD') return null;
      // `@{push}` names the remote's branch, `origin/main`.
      return ref === 'HEAD' ? out : out.slice(out.indexOf('/') + 1);
    },
    prBase: (args) => run(['gh', 'pr', 'view', ...args, '--json', 'baseRefName', '--jq', '.baseRefName']),
  };
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
    const way = await landingRefused(String(e.input?.command ?? ''), lookOn($));
    if (way) return { decision: 'deny', reason: way };
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
