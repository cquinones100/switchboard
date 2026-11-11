//! Stand-ins for the programs Switchboard runs, each as strict as the real
//! one about how it is called, so a test passes only when the code calls it
//! rightly: a stand-in that took anything once let every release ship
//! unable to install, since the stub releases ignored the temporary name
//! the installers ran them under and the real binary didn't.
//!
//! Each is a POSIX sh script, for dash on Linux and the bash 3.2 a Mac has
//! as /bin/sh, using nothing BSD's tools lack. It checks the call first,
//! refusing what the real program refuses (a name it doesn't answer to, an
//! option it doesn't have, an option missing its value, a value it doesn't
//! take, a missing argument or one too many, stdin it needs and wasn't
//! given) with the real program's words and exit status, then does what the
//! test says with the arguments as they were. A stand-in that is odd on
//! purpose (one that hangs, fails or answers nonsense, to show how the code
//! takes that) is still strict about the call: its oddness is in its answer.
//!
//! Every call a stand-in refuses is written down in `.refused` in the folder
//! it was installed in, since the code often takes a refusal quietly (a
//! background lookup that failed, a connector that seems down): a test that
//! passes while a stand-in refused a call would have passed with a wrong
//! call. `common::TestServer` fails a test whose stand-ins refused anything
//! (`refusals`), and so do the other homes the tests make.
//!
//! Used by the integration tests as `common::stand_in` and by the library's
//! own unit tests as `crate::stand_in` (src/lib.rs includes this file), so it
//! uses nothing but std. tests/stand_ins.rs shows each refusing a wrong call.

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

/// Writes `text` to `path` as a program, making its folder; gives the path.
pub fn install(path: &Path, text: &str) -> PathBuf {
    install_all(&[(path.to_path_buf(), text.to_string())]);
    path.to_path_buf()
}

/// Writes each of `programs` (a path and its text) as a program, making
/// their folders, all through one child process, so none is ever open for
/// writing in this one. A program written here and run at once fails now
/// and then with "Text file busy": Linux won't run a file that any process
/// has open for writing, and another test's thread starting a program just
/// then hands its child a copy of the open file, which it keeps until it
/// execs. The texts are staged in a folder of their own, never run, and the
/// child copies each into a new file (the old one taken away first, so a
/// copy still running is left alone) and makes it executable.
pub fn install_all(programs: &[(PathBuf, String)]) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let staging = std::env::temp_dir().join(format!(
        "sb-stand-in-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&staging).unwrap();
    let mut copy = std::process::Command::new("/bin/sh");
    // The system's own tools, whatever PATH a test gave this process (some
    // have only their stand-ins on it).
    copy.env("PATH", "/bin:/usr/bin").arg("-c").arg(
        r#"while [ $# -gt 0 ]; do rm -f "$2" && cp "$1" "$2" && chmod 755 "$2" || exit 1; shift 2; done"#,
    );
    copy.arg("sh");
    for (n, (path, text)) in programs.iter().enumerate() {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).unwrap();
        }
        let staged = staging.join(n.to_string());
        fs::write(&staged, text).unwrap();
        copy.arg(staged).arg(path);
    }
    let status = copy.status().unwrap();
    let _ = fs::remove_dir_all(&staging);
    let paths: Vec<&PathBuf> = programs.iter().map(|(path, _)| path).collect();
    assert!(status.success(), "couldn't install {paths:?}");
}

/// `text` quoted for sh, as one word whatever it holds.
pub fn sh_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// `path` quoted for sh.
fn quoted(path: &Path) -> String {
    sh_quote(&path.to_string_lossy())
}

/// Where the stand-ins in a folder write down the calls they refused.
pub const REFUSED: &str = ".refused";

/// What every stand-in starts with: the call as it came, and `wrong`, which
/// writes it down in `refused` (none: nowhere) with why it was refused.
fn prologue(refused: Option<&Path>) -> String {
    let file = refused.map_or("/dev/null".into(), quoted);
    format!(
        r#"stand_in_call="${{0##*/}} $*"
stand_in_refused={file}
# A call the real program would refuse, written down for the test to see.
wrong() {{
  printf '%s\n    %s\n' "$stand_in_call" "$1" >> "$stand_in_refused"
}}"#
    )
}

/// `template` with its prologue, writing refusals beside `program` when it
/// is installed there.
fn finish(template: &str, program: Option<&Path>) -> String {
    let refused = program.and_then(Path::parent).map(|dir| dir.join(REFUSED));
    template.replace("@PROLOGUE@", &prologue(refused.as_deref()))
}

/// Writes `template` as the program `name` in `bin`; gives its path.
fn install_in(bin: &Path, name: &str, template: &str) -> PathBuf {
    let path = bin.join(name);
    let text = finish(template, Some(&path));
    install(&path, &text)
}

/// The calls the stand-ins in `bin` refused, each with why, when any were.
pub fn refusals(bin: &Path) -> Option<String> {
    fs::read_to_string(bin.join(REFUSED))
        .ok()
        .filter(|t| !t.is_empty())
}

/// Fails unless the stand-ins in `bin` took every call made of them.
pub fn assert_none_refused(bin: &Path) {
    if let Some(refused) = refusals(bin) {
        panic!(
            "a stand-in in {} refused a call the code made, which the real program would refuse too:\n{refused}",
            bin.display()
        );
    }
}

/// The part of a script that checks the call, run first in a subshell so
/// whatever it refuses is written down with the words it was refused in,
/// then for real, setting what the rest reads.
const CHECKED: &str = r#"stand_in_why=$( (check "$@") 2>&1 >/dev/null)
stand_in_status=$?
if [ "$stand_in_status" != 0 ]; then
  printf '%s\n' "$stand_in_why" >&2
  wrong "$stand_in_why (exit $stand_in_status)"
  exit "$stand_in_status"
fi
check "$@""#;

// ── Claude Code ─────────────────────────────────────────────────────────────

/// How an option of a program parsed by commander (Claude Code's) takes its
/// value: none, one (the next argument, whatever it is), maybe one (the next
/// argument unless it looks like an option), or several (every argument up
/// to the next that looks like an option).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Takes {
    Nothing,
    Value,
    Maybe,
    Values,
}

/// Claude Code's options, as `claude --help` lists them in 2.1.319, and the
/// hidden ones it takes all the same (`--max-turns`, the prompt files).
const CLAUDE_OPTIONS: &[&str] = &[
    "--add-dir <directories...>",
    "--agent <agent>",
    "--agents <json-or-file>",
    "--allow-dangerously-skip-permissions",
    "--allowedTools, --allowed-tools <tools...>",
    "--append-system-prompt <prompt>",
    "--append-system-prompt-file <file>",
    "--autocompact <auto|tokens>",
    "--ax-screen-reader",
    "--bg, --background",
    "--bare",
    "--betas <betas...>",
    "--brief",
    "--chrome",
    "--cloud [description|session_id|url]",
    "-c, --continue",
    "--dangerously-skip-permissions",
    "-d, --debug [filter]",
    "--debug-file <path>",
    "--desktop",
    "--disable-slash-commands",
    "--disallowedTools, --disallowed-tools <tools...>",
    "--effort <level>",
    "--environment <environment_id>",
    "--exclude-dynamic-system-prompt-sections",
    "--fallback-model <model>",
    "--file <specs...>",
    "--fork-session",
    "--forward-subagent-text",
    "--from-pr [value]",
    "-h, --help",
    "--ide",
    "--include-hook-events",
    "--include-partial-messages",
    "--input-format <format>",
    "--json-schema <schema>",
    "--max-budget-usd <amount>",
    "--max-thinking-tokens <tokens>",
    "--max-turns <turns>",
    "--mcp-config <configs...>",
    "--model <model>",
    "-n, --name <name>",
    "--no-chrome",
    "--no-session-persistence",
    "--output-format <format>",
    "--permission-mode <mode>",
    "--permission-prompt-tool <tool>",
    "--permission-prompts <target>",
    "--plugin-dir <path>",
    "--plugin-url <url>",
    "-p, --print",
    "--prompt-suggestions [value]",
    "--remote-control [name]",
    "--remote-control-session-name-prefix <prefix>",
    "--replay-user-messages",
    "--restricted",
    "-r, --resume [value]",
    "--safe-mode",
    "--session-id <uuid>",
    "--setting-sources <sources>",
    "--settings <file-or-json>",
    "--strict-mcp-config",
    "--system-prompt <prompt>",
    "--system-prompt-file <file>",
    "--system-prompt-snapshot <on|off>",
    "--teleport [session]",
    "--tmux [mode]",
    "--tools <tools...>",
    "--verbose",
    "-v, --version",
    "-w, --worktree [name]",
];

/// The options whose value is one of a list, which commander checks.
const CLAUDE_CHOICES: &[(&str, &[&str])] = &[
    ("--input-format", &["text", "stream-json"]),
    ("--output-format", &["text", "json", "stream-json"]),
    (
        "--permission-mode",
        &[
            "acceptEdits",
            "auto",
            "bypassPermissions",
            "manual",
            "dontAsk",
            "plan",
        ],
    ),
    ("--permission-prompts", &["host", "none"]),
    (
        "--prompt-suggestions",
        &["true", "false", "1", "0", "yes", "no", "on", "off"],
    ),
    ("--system-prompt-snapshot", &["on", "off"]),
];

/// Claude Code's commands, besides the prompt.
const CLAUDE_COMMANDS: &[&str] = &[
    "agents",
    "attach",
    "auth",
    "auto-mode",
    "doctor",
    "gateway",
    "import",
    "install",
    "logs",
    "mcp",
    "plugin",
    "plugins",
    "purge",
    "respawn",
    "rm",
    "setup-token",
    "stop",
    "kill",
    "ultrareview",
    "update",
    "upgrade",
];

/// An option's names (`-p`, `--print`), the one commander knows it by (its
/// first long name), and how it takes its value, from its spec.
fn option_of(spec: &str) -> (Vec<&str>, &str, Takes) {
    let at = spec
        .find(" <")
        .or_else(|| spec.find(" ["))
        .unwrap_or(spec.len());
    let (names, value) = (&spec[..at], spec[at..].trim());
    let names: Vec<&str> = names.split(", ").collect();
    let long = names
        .iter()
        .copied()
        .find(|n| n.starts_with("--"))
        .unwrap_or(names[0]);
    let takes = if value.starts_with('<') && value.contains("...") {
        Takes::Values
    } else if value.starts_with('<') {
        Takes::Value
    } else if value.starts_with('[') {
        Takes::Maybe
    } else {
        Takes::Nothing
    };
    (names, long, takes)
}

/// The sh function `takes NAME`, which sets `kind` (flag, value, optional,
/// values, or empty for an option Claude Code doesn't have), `name` and
/// `spec`, from `CLAUDE_OPTIONS`.
fn claude_takes() -> String {
    let mut sh = String::from("takes() {\n  kind=\n  case $1 in\n");
    for spec in CLAUDE_OPTIONS {
        let (names, long, takes) = option_of(spec);
        let kind = match takes {
            Takes::Nothing => "flag",
            Takes::Value => "value",
            Takes::Maybe => "optional",
            Takes::Values => "values",
        };
        sh.push_str(&format!(
            "    {}) kind={kind} name={long} spec={} ;;\n",
            names.join("|"),
            sh_quote(spec)
        ));
    }
    sh.push_str("  esac\n}\n");
    sh
}

/// The sh function `choose OPTION VALUE`, refusing a value that isn't one of
/// the option's choices as commander does.
fn claude_choose() -> String {
    let mut sh = String::from("choose() {\n  case $1 in\n");
    for (option, choices) in CLAUDE_CHOICES {
        sh.push_str(&format!(
            "    {option})\n      case $2 in\n        {}) ;;\n        *) refuse \"option '$spec' argument '$2' is invalid. Allowed choices are {}.\" ;;\n      esac ;;\n",
            choices.join("|"),
            choices.join(", ")
        ));
    }
    sh.push_str("  esac\n}\n");
    sh
}

const CLAUDE: &str = r#"#!/bin/sh
# A stand-in for Claude Code (tests/common/stand_in.rs), as strict as it is
# about how it is called: its options and their values as commander reads
# them, at most one prompt, `claude mcp get <name>`, and with -p a prompt
# from its argument or stdin. A call it would refuse fails as it does; any
# other runs the test's answer at the end, with $CLAUDE_CALL saying which
# kind of call it was (print, interactive, mcp get, mcp list) and the
# arguments as they were.
@PROLOGUE@
claude_version=@VERSION@
refuse() {
  wrong "error: $1"
  printf 'error: %s\n' "$1" >&2
  exit 1
}
fail() {
  wrong "$1"
  printf '%s\n' "$1" >&2
  exit 1
}
no_answer() {
  fail "claude (a stand-in): it has no answer for \`claude $1\`"
}
@TAKES@
@CHOOSE@
given() {
  choose "$1" "$2"
  case $1 in
    --output-format) output=$2 ;;
    --input-format) input=$2 ;;
  esac
}
flag() {
  case $1 in
    --print) print=1 ;;
    --verbose) verbose=1 ;;
    --version)
      printf '%s (Claude Code)\n' "$claude_version"
      exit 0 ;;
    --help)
      printf 'Usage: claude [options] [command] [prompt]\n'
      exit 0 ;;
  esac
}
operand() {
  if [ -z "$command" ] && [ "$operands" = 0 ]; then
    case $1 in
      mcp)
        command=mcp
        return ;;
      @COMMANDS@) no_answer "$1" ;;
    esac
  fi
  operands=$((operands + 1))
}
mcp_operand() {
  if [ -n "$mcp" ]; then
    mcp_args=$((mcp_args + 1))
    return
  fi
  case $1 in
    get | list) mcp=$1 ;;
    add | add-json | add-from-claude-desktop | remove | reset-project-choices | serve) no_answer "mcp $1" ;;
    *) refuse "unknown command '$1'" ;;
  esac
}
check() {
  print= verbose= output=text input=text want= opt= spec= unknown= ended=
  command= mcp= mcp_args=0 operands=0
  for arg do
    if [ -n "$want" ]; then
      case $want in
        value)
          given "$opt" "$arg"
          want=
          continue ;;
        *)
          case $arg in
            -?*) want= ;;
            *)
              given "$opt" "$arg"
              [ "$want" = values ] || want=
              continue ;;
          esac ;;
      esac
    fi
    if [ -n "$command" ]; then
      case $arg in
        -h | --help)
          printf 'Usage: claude mcp [options] [command]\n'
          exit 0 ;;
        -?*) [ -n "$unknown" ] || unknown=$arg ;;
        *) mcp_operand "$arg" ;;
      esac
      continue
    fi
    if [ -n "$ended" ]; then
      operand "$arg"
      continue
    fi
    case $arg in
      --) ended=1 ;;
      --*=*)
        takes "${arg%%=*}"
        case $kind in
          value | optional) given "$name" "${arg#*=}" ;;
          values)
            given "$name" "${arg#*=}"
            opt=$name want=values ;;
          *) [ -n "$unknown" ] || unknown=$arg ;;
        esac ;;
      -?*)
        takes "$arg"
        case $kind in
          flag) flag "$name" ;;
          value | optional | values) opt=$name want=$kind ;;
          *) [ -n "$unknown" ] || unknown=$arg ;;
        esac ;;
      *) operand "$arg" ;;
    esac
  done
  [ "$want" != value ] || refuse "option '$spec' argument missing"
  [ -z "$unknown" ] || refuse "unknown option '$unknown'"
  if [ "$command" = mcp ]; then
    case $mcp in
      '') fail 'Usage: claude mcp [options] [command]' ;;
      get)
        [ "$mcp_args" -gt 0 ] || refuse "missing required argument 'name'"
        [ "$mcp_args" = 1 ] || refuse "too many arguments for 'get'. Expected 1 argument but got $mcp_args." ;;
      list)
        [ "$mcp_args" = 0 ] || refuse "too many arguments for 'list'. Expected 0 arguments but got $mcp_args." ;;
    esac
    CLAUDE_CALL="mcp $mcp"
    return
  fi
  [ "$operands" -le 1 ] || refuse "too many arguments. Expected 1 argument but got $operands."
  if [ -z "$print" ]; then
    CLAUDE_CALL=interactive
    return
  fi
  CLAUDE_CALL=print
  if [ "$output" = stream-json ] && [ -z "$verbose" ]; then
    fail 'Error: When using --print, --output-format=stream-json requires --verbose'
  fi
  if [ "$input" = stream-json ] && [ "$output" != stream-json ]; then
    fail 'Error: --input-format=stream-json requires output-format=stream-json.'
  fi
}
check "$@"
# With -p, Claude reads stdin whole (unless it is a terminal), the prompt
# when no argument gives one; kept for the answer, which reads it as stdin.
if [ "$CLAUDE_CALL" = print ]; then
  stdin_had=
  if [ ! -t 0 ]; then
    kept=$(mktemp "${TMPDIR:-/tmp}/claude-stdin.XXXXXX") || exit 1
    cat > "$kept"
    exec < "$kept"
    [ ! -s "$kept" ] || stdin_had=1
    rm -f "$kept"
  fi
  if [ "$operands" = 0 ] && [ -z "$stdin_had" ]; then
    fail 'Error: Input must be provided either through stdin or as a prompt argument when using --print'
  fi
fi
export CLAUDE_CALL
# The test's answer.
@ANSWER@
"#;

/// A stand-in Claude Code: `claude --version` says `version`, and a call it
/// takes runs `answer` (sh, with "$@" and `$CLAUDE_CALL`, and with -p the
/// prompt on stdin), which by default says nothing and succeeds.
#[derive(Clone, Debug)]
pub struct Claude {
    version: String,
    answer: String,
}

impl Default for Claude {
    fn default() -> Claude {
        Claude::new()
    }
}

impl Claude {
    pub fn new() -> Claude {
        Claude {
            version: "2.1.319".into(),
            answer: ":".into(),
        }
    }

    /// A Claude Code that isn't signed in, as one is in a home of a test's
    /// own: a prompt fails and asks for /login, and it knows no connector.
    pub fn signed_out() -> Claude {
        Claude::new().answer(
            r#"case $CLAUDE_CALL in
  'mcp get')
    printf 'No MCP server found with name: %s\n' "$3" >&2
    exit 1 ;;
  'mcp list') printf 'No MCP servers configured. Use `claude mcp add` to add a server.\n' ;;
  *)
    printf 'Not logged in · Please run /login\n'
    exit 1 ;;
esac"#,
        )
    }

    /// The version `claude --version` says.
    pub fn version(mut self, version: &str) -> Claude {
        self.version = version.into();
        self
    }

    /// What it does with a call it takes, in sh.
    pub fn answer(mut self, sh: &str) -> Claude {
        self.answer = sh.into();
        self
    }

    /// The script, which writes down nowhere what it refuses.
    pub fn script(&self) -> String {
        self.script_at(None)
    }

    fn script_at(&self, path: Option<&Path>) -> String {
        finish(CLAUDE, path)
            .replace("@VERSION@", &sh_quote(&self.version))
            .replace("@TAKES@", &claude_takes())
            .replace("@CHOOSE@", &claude_choose())
            .replace(
                "@COMMANDS@",
                &CLAUDE_COMMANDS
                    .iter()
                    .filter(|c| **c != "mcp")
                    .copied()
                    .collect::<Vec<_>>()
                    .join(" | "),
            )
            .replace("@ANSWER@", &self.answer)
    }

    /// Writes it as `claude` in `bin`, writing down what it refuses there;
    /// gives its path.
    pub fn install(&self, bin: &Path) -> PathBuf {
        let path = bin.join("claude");
        install(&path, &self.script_at(Some(&path)))
    }
}

// ── gh ──────────────────────────────────────────────────────────────────────

const GH: &str = r#"#!/bin/sh
# A stand-in for GitHub's gh (tests/common/stand_in.rs), as strict as it is
# about how it is called: the commands Switchboard runs (`gh auth token`, and
# `gh auth git-credential`, which git runs as a credential helper), with
# their flags as cobra reads them. A call it would refuse fails as gh does;
# any other runs the test's answer at the end, with $GH_CALL naming the
# command and the arguments as they were.
@PROLOGUE@
unknown_command() {
  wrong "unknown command \"$1\" for \"$2\""
  printf 'unknown command "%s" for "%s"\n\nUsage:  %s <command> [flags]\n' "$1" "$2" "$2" >&2
  exit 1
}
unknown_flag() {
  wrong "unknown flag $1 for \"$2\""
  case $1 in
    --*) printf 'unknown flag: %s\n' "${1%%=*}" >&2 ;;
    *) printf "unknown shorthand flag: '%s' in %s\n" "$(printf '%s' "${1#-}" | cut -c1)" "$1" >&2 ;;
  esac
  printf '\nUsage:  %s [flags]\n' "$2" >&2
  exit 1
}
needs() {
  wrong "flag needs an argument: $1"
  case $1 in
    --*) printf 'flag needs an argument: %s\n' "$1" >&2 ;;
    *) printf "flag needs an argument: '%s' in %s\n" "${1#-}" "$1" >&2 ;;
  esac
  exit 1
}
no_answer() {
  wrong "gh (a stand-in): it has no answer for \`gh $1\`"
  printf 'gh (a stand-in): it has no answer for `gh %s`\n' "$1" >&2
  exit 1
}
check() {
  case ${1-} in
    --version)
      printf 'gh version 2.83.0 (stand-in)\n'
      exit 0 ;;
    '' | --help | help)
      printf 'Work seamlessly with GitHub from the command line.\n\nUSAGE\n  gh <command> <subcommand> [flags]\n'
      exit 0 ;;
    auth) shift ;;
    -*) unknown_flag "$1" gh ;;
    agent-task | alias | api | attestation | browse | cache | co | codespace | completion | config | copilot | extension | gist | gpg-key | issue | label | org | pr | preview | project | release | repo | ruleset | run | search | secret | ssh-key | status | variable | workflow) no_answer "$1" ;;
    *) unknown_command "$1" gh ;;
  esac
  case ${1-} in
    token)
      shift
      GH_CALL='auth token'
      while [ $# -gt 0 ]; do
        case $1 in
          -h | --hostname | -u | --user)
            [ $# -ge 2 ] || needs "$1"
            shift ;;
          --hostname=* | --user=* | --no-refresh) ;;
          --help)
            printf 'USAGE\n  gh auth token [flags]\n'
            exit 0 ;;
          -?*) unknown_flag "$1" 'gh auth token' ;;
          *) unknown_command "$1" 'gh auth token' ;;
        esac
        shift
      done ;;
    git-credential)
      shift
      GH_CALL='auth git-credential'
      if [ $# != 1 ]; then
        wrong "accepts 1 arg(s), received $#"
        printf 'accepts 1 arg(s), received %s\n' "$#" >&2
        exit 1
      fi
      case $1 in
        get | store | erase) ;;
        -?*) unknown_flag "$1" 'gh auth git-credential' ;;
        *)
          wrong "\"$1\" operation not supported"
          printf 'gh auth git-credential: "%s" operation not supported\n' "$1" >&2
          exit 1 ;;
      esac ;;
    '' | --help)
      printf 'Authenticate gh and git with GitHub\n\nUSAGE\n  gh auth <command> [flags]\n'
      exit 0 ;;
    login | logout | refresh | setup-git | status | switch) no_answer "auth $1" ;;
    -?*) unknown_flag "$1" 'gh auth' ;;
    *) unknown_command "$1" 'gh auth' ;;
  esac
}
check "$@"
export GH_CALL
# The test's answer.
@ANSWER@
"#;

/// What gh answers when it isn't signed in.
const GH_SIGNED_OUT: &str = r#"case $GH_CALL in
  'auth token')
    printf 'no oauth token found for github.com\n' >&2
    exit 1 ;;
  *) exit 1 ;;
esac"#;

/// A stand-in gh: a call it takes runs `answer` (sh, with "$@" and
/// `$GH_CALL`); by default it isn't signed in.
#[derive(Clone, Debug)]
pub struct Gh {
    answer: String,
}

impl Default for Gh {
    fn default() -> Gh {
        Gh::new()
    }
}

impl Gh {
    pub fn new() -> Gh {
        Gh {
            answer: GH_SIGNED_OUT.into(),
        }
    }

    /// Signed in while `file` is there: `gh auth token` says a token, and
    /// the credential helper answers with it.
    pub fn signed_in_while(file: &Path) -> Gh {
        let file = quoted(file);
        // "$@" is `auth token ...`, or `auth git-credential <operation>`
        // with git's lines on stdin.
        Gh::new().answer(&format!(
            "[ -e {file} ] || {{\n{GH_SIGNED_OUT}\n}}\ncase $GH_CALL in\n  'auth token') printf 'gho_stand_in\\n' ;;\n  'auth git-credential')\n    cat > /dev/null\n    [ \"$3\" != get ] || printf 'username=x-access-token\\npassword=gho_stand_in\\n' ;;\nesac"
        ))
    }

    /// What it does with a call it takes, in sh.
    pub fn answer(mut self, sh: &str) -> Gh {
        self.answer = sh.into();
        self
    }

    /// The script, which writes down nowhere what it refuses.
    pub fn script(&self) -> String {
        finish(GH, None).replace("@ANSWER@", &self.answer)
    }

    /// Writes it as `gh` in `bin`; gives its path.
    pub fn install(&self, bin: &Path) -> PathBuf {
        install_in(bin, "gh", &GH.replace("@ANSWER@", &self.answer))
    }
}

// ── Switchboard's own binary, as a release ──────────────────────────────────

const RELEASE: &str = r#"#!/bin/sh
# A stand-in for a release of switchboard (tests/common/stand_in.rs). Like
# every release up to 0.2.0 it is switchboard only under that name, reading
# any other as a command it doesn't have, which is how an installer that ran
# it under a temporary name failed; and it has the commands an installer and
# the server ask of it: --version, setup with its parts, and server
# --handover-version.
@PROLOGUE@
parts() {
  shift
  for part do
    case $part in
      --check | --refresh | --move | --parts | -h | --help | plugin | claude | claude-md | claude_md | skills | terminal | leftovers) ;;
      *)
        wrong "Unknown part: $part"
        printf 'Unknown part: %s\n' "$part" >&2
        printf '  parts: plugin claude claude-md skills terminal leftovers\n' >&2
        exit 1 ;;
    esac
  done
}
called=${0##*/}
if [ "$called" != switchboard ]; then
  wrong "run as $called, which no release answers to"
  printf 'switchboard: no command named %s (switchboard help lists them)\n' "$called" >&2
  exit 2
fi
case ${1-} in
  --version | -V | version)
    printf 'switchboard %s\n' @VERSION@ ;;
  help | -h | --help)
    printf 'Usage: switchboard              start the workspace, or attach to it\n' ;;
  setup)
    parts "$@"
    @SETUP@ ;;
  server)
    if [ "$*" != 'server --handover-version' ]; then
      wrong 'it has no server to run'
      printf 'switchboard (a stand-in): it has no server to run\n' >&2
      exit 1
    fi
    @HANDOVER@ ;;
  '')
    wrong 'it has no workspace to start'
    printf 'switchboard (a stand-in): it has no workspace to start\n' >&2
    exit 1 ;;
  *)
    wrong "no command named $1"
    printf 'switchboard: no command named %s (switchboard help lists them)\n' "$1" >&2
    exit 2 ;;
esac
"#;

/// A stand-in release of switchboard, saying `version`: its setup runs
/// `setup` (sh, with "$@" as it was: `setup` and its parts), and `server
/// --handover-version` answers as a build from before upgrades did (its
/// usage, exit 2) unless `handover` gives the format it takes over in.
#[derive(Clone, Debug)]
pub struct Release {
    version: String,
    setup: String,
    handover: Option<String>,
}

impl Release {
    pub fn new(version: &str) -> Release {
        Release {
            version: version.into(),
            setup: ":".into(),
            handover: None,
        }
    }

    /// What its setup does, in sh.
    pub fn on_setup(mut self, sh: &str) -> Release {
        self.setup = sh.into();
        self
    }

    /// The handover format it says it takes over in.
    pub fn handover(mut self, format: impl std::fmt::Display) -> Release {
        self.handover = Some(format.to_string());
        self
    }

    /// The script, which writes down nowhere what it refuses: as a file
    /// inside an archive, run where the test doesn't look.
    pub fn script(&self) -> String {
        self.script_at(None)
    }

    fn script_at(&self, path: Option<&Path>) -> String {
        // A build from before upgrades refused the question, as a release
        // is meant to here, so it isn't written down.
        let handover = match &self.handover {
            Some(format) => format!("printf '%s\\n' {}", sh_quote(format)),
            None => "printf 'Usage: switchboard server\\n' >&2\n    exit 2".into(),
        };
        finish(RELEASE, path)
            .replace("@VERSION@", &sh_quote(&self.version))
            .replace("@SETUP@", &self.setup)
            .replace("@HANDOVER@", &handover)
    }

    /// Writes it at `path`, writing down what it refuses beside it; gives
    /// the path.
    pub fn install(&self, path: &Path) -> PathBuf {
        install(path, &self.script_at(Some(path)))
    }
}

// ── The desktop's tools ─────────────────────────────────────────────────────

/// The tools that put text on the clipboard and read it back, on each
/// desktop: macOS's, Wayland's, X's (two), Windows' under WSL.
pub const CLIPBOARD: &[&str] = &[
    "pbcopy",
    "pbpaste",
    "wl-copy",
    "wl-paste",
    "xclip",
    "xsel",
    "clip.exe",
    "powershell.exe",
];

/// The tools that open a file or link on each desktop: macOS's, the Linux
/// desktops', and under WSL Windows' own (`wslpath` naming a file as
/// Windows does, for `explorer.exe`), which a test must never reach for
/// real, since it opens on the desktop of the computer the tests run on.
pub const OPENERS: &[&str] = &[
    "open",
    "xdg-open",
    "wslview",
    "gio",
    "wslpath",
    "explorer.exe",
];

/// Chrome and Chromium, by every name Switchboard looks for them by.
pub const CHROMES: &[&str] = &[
    "google-chrome",
    "google-chrome-stable",
    "chromium",
    "chromium-browser",
];

/// Each desktop tool's check of its call, in sh: a function `check`, and
/// what it does with a call it takes, after `said` has written it down.
fn desktop_check(name: &str) -> &'static str {
    match name {
        "pbcopy" | "pbpaste" => {
            r#"usage() {
  printf 'usage: %s [-help] [-pboard {general | ruler | find | font}] [-Prefer {txt | rtf | ps}]\n' "$tool" >&2
  exit 1
}
check() {
  while [ $# -gt 0 ]; do
    case $1 in
      -pboard)
        [ $# -ge 2 ] || usage
        case $2 in general | ruler | find | font) ;; *) usage ;; esac
        shift ;;
      -Prefer)
        [ $# -ge 2 ] || usage
        case $2 in txt | rtf | ps) ;; *) usage ;; esac
        shift ;;
      *) usage ;;
    esac
    shift
  done
}
act() {
  if [ "$tool" = pbcopy ]; then cat > "$clipboard"; else cat "$clipboard" 2>/dev/null; fi
}"#
        }
        "wl-copy" | "wl-paste" => {
            r#"bad() {
  case $1 in
    --*) printf "%s: unrecognized option '%s'\n" "$tool" "$1" >&2 ;;
    *) printf "%s: invalid option -- '%s'\n" "$tool" "$(printf '%s' "${1#-}" | cut -c1)" >&2 ;;
  esac
  printf "Try '%s --help' for more information.\n" "$tool" >&2
  exit 1
}
check() {
  text= texted= kind=
  while [ $# -gt 0 ]; do
    case $tool:$1 in
      wl-copy:-o | wl-copy:--paste-once | wl-copy:-f | wl-copy:--foreground | wl-copy:-c | wl-copy:--clear | wl-copy:-n | wl-copy:--trim-newline | wl-copy:--regular | wl-paste:-n | wl-paste:--no-newline | wl-paste:-l | wl-paste:--list-types | *:-p | *:--primary) ;;
      *:-t | *:--type | *:-s | *:--seat)
        if [ $# -lt 2 ]; then
          printf "%s: option requires an argument -- '%s'\n" "$tool" "${1#-}" >&2
          printf "Try '%s --help' for more information.\n" "$tool" >&2
          exit 1
        fi
        [ "$1" != -t ] && [ "$1" != --type ] || kind=$2
        shift ;;
      *:--type=*) kind=${1#*=} ;;
      *:--seat=*) ;;
      wl-copy:--)
        shift
        text="$*" texted=1
        return ;;
      *:-?*) bad "$1" ;;
      wl-copy:*)
        text="${text:+$text }$1" texted=1 ;;
      *)
        printf "%s: unexpected argument '%s'\n" "$tool" "$1" >&2
        exit 1 ;;
    esac
    shift
  done
}
act() {
  if [ "$tool" = wl-copy ]; then
    if [ -n "$texted" ]; then printf '%s\n' "$text" > "$clipboard"; else cat > "$clipboard"; fi
  elif [ -n "$kind" ] && [ "$kind" != text/plain ]; then
    printf 'No suitable type of content copied\n' >&2
    exit 1
  else
    cat "$clipboard" 2>/dev/null
  fi
}"#
        }
        "xclip" => {
            r#"check() {
  out= files= target=
  while [ $# -gt 0 ]; do
    case $1 in
      -i | -in | -f | -filter | -r | -rmlastnl | -silent | -quiet | -verbose | -noutf8 | -sensitive) ;;
      -o | -out) out=1 ;;
      -selection | -sel | -t | -target | -l | -loops | -d | -display)
        if [ $# -lt 2 ]; then
          printf 'xclip: %s requires an argument\n' "$1" >&2
          exit 1
        fi
        case $1 in
          -selection | -sel)
            case $2 in
              primary | secondary | clipboard | buffer-cut | p | s | c | b) ;;
              *)
                printf 'xclip: %s: not a selection\n' "$2" >&2
                exit 1 ;;
            esac ;;
          -t | -target) target=$2 ;;
        esac
        shift ;;
      -version | -h | -help) exit 0 ;;
      *)
        if [ ! -e "$1" ]; then
          printf 'xclip: %s: No such file or directory\n' "$1" >&2
          exit 1
        fi
        files="$files $1" ;;
    esac
    shift
  done
}
act() {
  if [ -z "$out" ]; then
    cat $files > "$clipboard"
  elif [ -n "$target" ] && [ "$target" != UTF8_STRING ] && [ "$target" != STRING ] && [ "$target" != text/plain ]; then
    printf 'Error: target %s not available\n' "$target" >&2
    exit 1
  else
    cat "$clipboard" 2>/dev/null
  fi
}"#
        }
        "xsel" => {
            r#"usage() {
  printf 'Usage: xsel [options]\nManipulate the X selection.\n' >&2
  exit 1
}
check() {
  out=
  while [ $# -gt 0 ]; do
    case $1 in
      -a | --append | -f | --follow | -z | --zeroflush | -i | --input | -c | --clear | -d | --delete | -p | --primary | -s | --secondary | -b | --clipboard | -k | --keep | -x | --exchange | -n | --nodetach | -v | --verbose | --trim | --keep-crlf) ;;
      -o | --output) out=1 ;;
      -t | --selectionTimeout | -l | --logfile | --display | -w | --windowName)
        [ $# -ge 2 ] || usage
        shift ;;
      -h | --help | --version) exit 0 ;;
      *) usage ;;
    esac
    shift
  done
}
act() {
  if [ -n "$out" ]; then cat "$clipboard" 2>/dev/null; else cat > "$clipboard"; fi
}"#
        }
        "clip.exe" => {
            r#"check() {
  if [ $# -gt 0 ]; then
    if [ "$*" = '/?' ]; then exit 0; fi
    printf "ERROR: Invalid argument/option - '%s'.\nType \"CLIP /?\" for usage.\n" "$1" >&2
    exit 1
  fi
}
act() {
  cat > "$clipboard"
}"#
        }
        "powershell.exe" => {
            r#"check() {
  ran=
  while [ $# -gt 0 ]; do
    case $1 in
      -NoProfile | -NoLogo | -NonInteractive | -Sta) ;;
      -Command)
        if [ $# -lt 2 ]; then
          printf "Missing argument for parameter 'Command'.\n" >&2
          exit 64
        fi
        ran=$2
        shift ;;
      *)
        printf "The argument '%s' is not recognized as the name of a script file.\n" "$1" >&2
        exit 64 ;;
    esac
    shift
  done
  case $ran in
    Get-Clipboard) ;;
    # src/platform/linux.rs's WINDOWS_CLIPBOARD_PNG: the clipboard's
    # picture, here the PNG beside the clipboard's text, if there is one.
    *'[System.Windows.Forms.Clipboard]::GetImage()'*'[Console]::OpenStandardOutput()'*) ;;
    *)
      printf 'powershell.exe (a stand-in): it has no answer for %s\n' "$ran" >&2
      exit 1 ;;
  esac
}
act() {
  case $ran in
    Get-Clipboard) cat "$clipboard" 2>/dev/null ;;
    *)
      [ -f "$clipboard.png" ] || exit 1
      cat "$clipboard.png" ;;
  esac
}"#
        }
        "open" => {
            r#"usage() {
  printf 'Usage: open [-e] [-t] [-f] [-W] [-R] [-n] [-g] [-h] [-s <partial SDK name>][-b <bundle identifier>] [-a <application>] [-u URL] [filenames] [--args arguments]\n' >&2
  exit 1
}
there() {
  case $1 in
    *://* | mailto:*) ;;
    *)
      if [ ! -e "$1" ]; then
        printf 'The file %s does not exist.\n' "$1" >&2
        exit 1
      fi ;;
  esac
}
check() {
  targets=0 app=
  while [ $# -gt 0 ]; do
    case $1 in
      --args) return ;;
      --env | --stdin | --stdout | --stderr)
        [ $# -ge 2 ] || usage
        shift ;;
      --hide | --background | --fresh | --new | --wait-apps | --reveal | --header) ;;
      --*)
        printf "open: unrecognized option \`%s'\n" "$1" >&2
        usage ;;
      -?*)
        letters=${1#-}
        while [ -n "$letters" ]; do
          letter=${letters%"${letters#?}"}
          letters=${letters#?}
          case $letter in
            e | t | f | F | W | R | n | g | j | h | D) ;;
            a | b | s | u)
              if [ -n "$letters" ]; then
                value=$letters letters=
              else
                [ $# -ge 2 ] || usage
                value=$2
                shift
              fi
              case $letter in a | b) app=$value ;; u) targets=$((targets + 1)) ;; esac ;;
            *)
              printf 'open: invalid option -- %s\n' "$letter" >&2
              usage ;;
          esac
        done ;;
      *)
        there "$1"
        targets=$((targets + 1)) ;;
    esac
    shift
  done
  [ "$targets" -gt 0 ] || [ -n "$app" ] || usage
}
act() {
  :
}"#
        }
        "xdg-open" | "wslview" => {
            r#"check() {
  if [ $# = 0 ]; then
    printf '%s: file or URL argument missing\n' "$tool" >&2
    printf "Try '%s --help' for more information.\n" "$tool" >&2
    exit 1
  fi
  case $1 in
    --help | --manual | --version | -h | -v) exit 0 ;;
    -*)
      printf "%s: unexpected option '%s'\n" "$tool" "$1" >&2
      printf "Try '%s --help' for more information.\n" "$tool" >&2
      exit 1 ;;
  esac
  if [ $# != 1 ]; then
    printf "%s: unexpected argument '%s'\n" "$tool" "$2" >&2
    printf "Try '%s --help' for more information.\n" "$tool" >&2
    exit 1
  fi
  case $1 in
    *://* | mailto:*) ;;
    *)
      if [ ! -e "$1" ]; then
        printf "%s: file '%s' does not exist\n" "$tool" "$1" >&2
        exit 2
      fi ;;
  esac
}
act() {
  :
}"#
        }
        "gio" => {
            r#"usage() {
  printf 'Usage:\n  gio COMMAND [ARGS…]\n' >&2
  exit 1
}
check() {
  case ${1-} in
    open) shift ;;
    help | --help | version | --version) exit 0 ;;
    '') usage ;;
    *)
      printf 'gio: Unknown command %s\n\n' "$1" >&2
      usage ;;
  esac
  if [ $# = 0 ]; then
    printf 'gio: No locations given\n\n' >&2
    usage
  fi
  for location do
    case $location in
      -h | --help) exit 0 ;;
      -?*)
        printf 'gio: Unknown option %s\n\n' "$location" >&2
        usage ;;
      *://* | mailto:*) ;;
      *)
        if [ ! -e "$location" ]; then
          printf 'gio: file://%s: Error when getting information for file “%s”: No such file or directory\n' "$location" "$location" >&2
          exit 2
        fi ;;
    esac
  done
}
act() {
  :
}"#
        }
        "wslpath" => {
            r#"usage() {
  printf 'Usage:\n    -a    force result to absolute path format\n    -u    translate from a Windows path to a WSL path (default)\n    -w    translate from a WSL path to a Windows path\n    -m    translate from a WSL path to a Windows path, with forward slashes\n' >&2
  exit 1
}
check() {
  to=u
  while [ $# -gt 0 ]; do
    case $1 in
      -a) ;;
      -u | -w | -m) to=${1#-} ;;
      -?*) usage ;;
      *) break ;;
    esac
    shift
  done
  [ $# = 1 ] || usage
  path=$1
}
act() {
  case $to in
    u) printf '%s\n' "$path" ;;
    *) printf '\\\\wsl.localhost\\Stand-in%s\n' "$(printf '%s' "$path" | tr / '\\')" ;;
  esac
}"#
        }
        // Explorer takes one thing to open, and exits 1 whether or not it
        // opened it.
        "explorer.exe" => {
            r#"check() {
  if [ $# != 1 ]; then
    exit 1
  fi
}
act() {
  exit 1
}"#
        }
        // Chrome takes every switch, knowing it or not, and every link.
        "google-chrome" | "google-chrome-stable" | "chromium" | "chromium-browser" => {
            r#"check() {
  :
}
act() {
  :
}"#
        }
        "osascript" => {
            r#"usage() {
  printf 'usage: osascript [-l language] [-e script] [-i] [-s {ehso}] [programfile] [argument ...]\n' >&2
  exit 1
}
check() {
  scripted=
  while [ $# -gt 0 ]; do
    case $1 in
      -e)
        [ $# -ge 2 ] || usage
        scripted=1
        shift ;;
      -l)
        [ $# -ge 2 ] || usage
        case $2 in
          AppleScript | JavaScript) ;;
          *)
            printf "osascript: invalid language '%s'\n" "$2" >&2
            exit 1 ;;
        esac
        shift ;;
      -s)
        [ $# -ge 2 ] || usage
        shift ;;
      -i) ;;
      --)
        shift
        break ;;
      -?*)
        printf 'osascript: illegal option -- %s\n' "$(printf '%s' "${1#-}" | cut -c1)" >&2
        usage ;;
      *) break ;;
    esac
    shift
  done
  if [ -z "$scripted" ] && [ $# -gt 0 ] && [ ! -e "$1" ]; then
    printf "osascript: %s: No such file or directory\n" "$1" >&2
    exit 1
  fi
}
act() {
  :
}"#
        }
        // macOS's: it holds the computer awake while the process -w names
        // runs, or for -t's seconds, or until it is ended.
        "caffeinate" => {
            r#"usage() {
  printf 'usage: caffeinate [-disu] [-t timeout] [-w Process ID] [command arguments...]\n' >&2
  exit 1
}
check() {
  watched= timeout=
  OPTIND=1
  while getopts ':dimsut:w:' opt; do
    case $opt in
      d | i | m | s | u) ;;
      t) timeout=$OPTARG ;;
      w) watched=$OPTARG ;;
      :)
        printf 'caffeinate: option requires an argument -- %s\n' "$OPTARG" >&2
        usage ;;
      *)
        printf 'caffeinate: illegal option -- %s\n' "$OPTARG" >&2
        usage ;;
    esac
  done
  shift $((OPTIND - 1))
  if [ $# -gt 0 ]; then
    printf 'caffeinate (a stand-in): it runs no command\n' >&2
    exit 1
  fi
}
act() {
  printf 'caffeinate is %s\n' "$$" >> "$log"
  if [ -n "$watched" ]; then
    while kill -0 "$watched" 2>/dev/null; do sleep 1; done
  elif [ -n "$timeout" ]; then
    sleep "$timeout"
  else
    while :; do sleep 1; done
  fi
}"#
        }
        other => panic!("no stand-in for {other}"),
    }
}

const DESKTOP: &str = r#"#!/bin/sh
# A stand-in for @TOOL@ (tests/common/stand_in.rs), as strict as it is about
# how it is called; a call it takes is written down in its log, as the
# tool's name and its arguments, and done with the test's clipboard file.
@PROLOGUE@
tool=@TOOL@
log=@LOG@
clipboard=@CLIPBOARD@
@CHECK@
@CHECKED@
printf '%s %s\n' "$tool" "$*" >> "$log"
act
"#;

/// Stand-ins for the desktop's tools in `tools` (names from `CLIPBOARD`,
/// `OPENERS`, `CHROMES`, `osascript` or `caffeinate`), in `bin`: each
/// writes each call it takes to `log`, a line of its name and arguments,
/// and keeps what is copied in `clipboard`, which pasting reads;
/// `caffeinate` writes `caffeinate is <pid>` after its call, and holds
/// until ended, as the real one does.
pub fn desktop(bin: &Path, tools: &[&str], log: &Path, clipboard: &Path) {
    let programs: Vec<(PathBuf, String)> = tools
        .iter()
        .map(|tool| {
            let script = DESKTOP
                .replace("@TOOL@", &sh_quote(tool))
                .replace("@LOG@", &quoted(log))
                .replace("@CLIPBOARD@", &quoted(clipboard))
                .replace("@CHECK@", desktop_check(tool))
                .replace("@CHECKED@", CHECKED);
            let path = bin.join(tool);
            let text = finish(&script, Some(&path));
            (path, text)
        })
        .collect();
    install_all(&programs);
}

// ── macOS's Keychain ────────────────────────────────────────────────────────

const SECURITY: &str = r#"#!/bin/sh
# A stand-in for macOS's security (tests/common/stand_in.rs), as strict as it
# is about how it is called: find-generic-password and add-generic-password,
# with their options. The keychain is a file of the test's own, a line an
# item: service, account and password, separated by the unit separator.
@PROLOGUE@
keychain=@KEYCHAIN@
sep=$(printf '\037')
usage() {
  printf 'Usage: security %s [-a account] [-s service] [options...] [keychain...]\n' "$command" >&2
  exit 2
}
check() {
  case ${1-} in
    find-generic-password | add-generic-password) command=$1 ;;
    '')
      printf 'security (a stand-in): it has no interactive mode\n' >&2
      exit 1 ;;
    -*)
      printf 'security: illegal option -- %s\n' "${1#-}" >&2
      exit 2 ;;
    *)
      printf 'security: unknown command "%s"\n' "$1" >&2
      exit 1 ;;
  esac
  shift
  account= service= password= show= update= got_password=
  while [ $# -gt 0 ]; do
    case $command:$1 in
      *:-a | *:-s | *:-l | *:-c | *:-C | *:-D | *:-G | *:-j | add-generic-password:-T | add-generic-password:-X | add-generic-password:-w)
        [ $# -ge 2 ] || usage
        case $1 in
          -a) account=$2 ;;
          -s) service=$2 ;;
          -w) password=$2 got_password=1 ;;
        esac
        shift ;;
      find-generic-password:-g) ;;
      find-generic-password:-w) show=1 ;;
      add-generic-password:-U) update=1 ;;
      add-generic-password:-A) ;;
      *:-h) usage ;;
      *:-?*)
        printf 'security: illegal option -- %s\n' "$(printf '%s' "${1#-}" | cut -c1)" >&2
        usage ;;
      *)
        if [ ! -e "$1" ]; then
          printf 'security: %s: The specified keychain could not be found.\n' "$1" >&2
          exit 50
        fi ;;
    esac
    shift
  done
  if [ "$command" = add-generic-password ] && [ -z "$got_password" ]; then
    printf 'security (a stand-in): it has no terminal to ask the password on\n' >&2
    exit 1
  fi
}
@CHECKED@
# An item's line, by its service and (when given) account.
found() {
  [ -f "$keychain" ] || return 1
  while IFS=$sep read -r s a p; do
    if [ "$s" = "$service" ] && { [ -z "$account" ] || [ "$a" = "$account" ]; }; then
      found_password=$p
      return 0
    fi
  done < "$keychain"
  return 1
}
case $command in
  find-generic-password)
    if ! found; then
      printf 'security: SecKeychainSearchCopyNext: The specified item could not be found in the keychain.\n' >&2
      exit 44
    fi
    if [ -n "$show" ]; then
      printf '%s\n' "$found_password"
    else
      printf 'keychain: "%s"\nclass: "genp"\nattributes:\n    "acct"<blob>="%s"\n    "svce"<blob>="%s"\n' "$keychain" "$account" "$service"
    fi ;;
  add-generic-password)
    if found; then
      if [ -z "$update" ]; then
        printf 'security: SecKeychainItemCreateFromContent (<default>): The specified item already exists in the keychain.\n' >&2
        exit 45
      fi
      grep -v "^$service$sep$account$sep" "$keychain" > "$keychain.new"
      mv "$keychain.new" "$keychain"
    fi
    printf '%s%s%s%s%s\n' "$service" "$sep" "$account" "$sep" "$password" >> "$keychain" ;;
esac
"#;

/// A stand-in for macOS's `security` in `bin`, its Keychain the file
/// `keychain`, which `keychain_item` adds to.
pub fn security(bin: &Path, keychain: &Path) -> PathBuf {
    install_in(
        bin,
        "security",
        &SECURITY
            .replace("@KEYCHAIN@", &quoted(keychain))
            .replace("@CHECKED@", CHECKED),
    )
}

/// An item in a stand-in Keychain.
pub fn keychain_item(keychain: &Path, service: &str, account: &str, password: &str) {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(keychain)
        .unwrap();
    writeln!(file, "{service}\x1f{account}\x1f{password}").unwrap();
}

// ── uname and sysctl ────────────────────────────────────────────────────────

const UNAME: &str = r#"#!/bin/sh
# A stand-in for uname (tests/common/stand_in.rs) on a computer of the test's
# choosing: GNU's on Linux, BSD's on a Mac, each refusing what it refuses.
@PROLOGUE@
os=@OS@
arch=@ARCH@
bad() {
  if [ "$os" = Darwin ]; then
    printf 'uname: illegal option -- %s\nusage: uname [-amnoprsv]\n' "$1" >&2
  else
    printf "uname: invalid option -- '%s'\nTry 'uname --help' for more information.\n" "$1" >&2
  fi
  exit 1
}
check() {
  fields=
  for arg do
    case $arg in
      --help | --version)
        [ "$os" != Darwin ] || bad "${arg#-}"
        exit 0 ;;
      -?*)
        letters=${arg#-}
        while [ -n "$letters" ]; do
          letter=${letters%"${letters#?}"}
          letters=${letters#?}
          case $letter in
            a | s | n | r | v | m | p) fields="$fields$letter" ;;
            i | o) [ "$os" = Darwin ] && [ "$letter" = i ] && bad "$letter"; fields="$fields$letter" ;;
            *) bad "$letter" ;;
          esac
        done ;;
      *)
        if [ "$os" = Darwin ]; then
          printf 'usage: uname [-amnoprsv]\n' >&2
        else
          printf "uname: extra operand '%s'\nTry 'uname --help' for more information.\n" "$arg" >&2
        fi
        exit 1 ;;
    esac
  done
  [ -n "$fields" ] || fields=s
}
@CHECKED@
has() {
  case $fields in *a* | *"$1"*) return 0 ;; esac
  return 1
}
said=
put() {
  said="${said:+$said }$1"
}
has s && put "$os"
has n && put stand-in
has r && put 1.0.0
has v && put '#1'
has m && put "$arch"
has p && put "$arch"
has i && put "$arch"
has o && { [ "$os" = Darwin ] && put Darwin || put GNU/Linux; }
printf '%s\n' "$said"
"#;

const SYSCTL: &str = r#"#!/bin/sh
# A stand-in for macOS's sysctl (tests/common/stand_in.rs), which knows one
# value, sysctl.proc_translated (1 under Rosetta, 0 on Apple silicon, and
# unknown on an Intel Mac), and refuses what BSD's sysctl refuses.
@PROLOGUE@
translated=@TRANSLATED@
usage() {
  printf 'usage: sysctl [-bdehiNnoqx] name[=value] ...\n       sysctl [-bdehNnoqx] -a\n' >&2
  exit 1
}
check() {
  quiet= bare= every= names=0
  for arg do
    case $arg in
      -?*)
        letters=${arg#-}
        while [ -n "$letters" ]; do
          letter=${letters%"${letters#?}"}
          letters=${letters#?}
          case $letter in
            i) quiet=1 ;;
            n) bare=1 ;;
            a) every=1 ;;
            b | d | e | h | N | o | q | x) ;;
            *)
              printf 'sysctl: illegal option -- %s\n' "$letter" >&2
              usage ;;
          esac
        done ;;
      *) names=$((names + 1)) ;;
    esac
  done
  [ "$names" -gt 0 ] || [ -n "$every" ] || usage
}
@CHECKED@
status=0
for arg do
  case $arg in
    -*) continue ;;
    sysctl.proc_translated)
      if [ -n "$translated" ]; then
        if [ -n "$bare" ]; then printf '%s\n' "$translated"; else printf '%s: %s\n' "$arg" "$translated"; fi
        continue
      fi ;;
  esac
  if [ -z "$quiet" ]; then
    printf "sysctl: unknown oid '%s'\n" "$arg" >&2
    status=1
  fi
done
exit $status
"#;

/// Stand-ins for `uname`, saying `os` (`Linux`, `Darwin`) and `arch`, and
/// for macOS's `sysctl`, whose `sysctl.proc_translated` is `translated`
/// (empty: an Intel Mac, which has no such value), in `bin`.
pub fn platform(bin: &Path, os: &str, arch: &str, translated: &str) {
    install_in(
        bin,
        "uname",
        &UNAME
            .replace("@OS@", &sh_quote(os))
            .replace("@ARCH@", &sh_quote(arch))
            .replace("@CHECKED@", CHECKED),
    );
    install_in(
        bin,
        "sysctl",
        &SYSCTL
            .replace("@TRANSLATED@", &sh_quote(translated))
            .replace("@CHECKED@", CHECKED),
    );
}

// ── cargo ───────────────────────────────────────────────────────────────────

const CARGO: &str = r#"#!/bin/sh
# A stand-in for cargo (tests/common/stand_in.rs), with only `cargo build`
# and the options cargo has for it, refusing as cargo does what it refuses,
# and needing a Cargo.toml here or above, as cargo does.
@PROLOGUE@
unexpected() {
  printf "error: unexpected argument '%s' found\n\nUsage: cargo build [OPTIONS]\n\nFor more information, try '--help'.\n" "$1" >&2
  exit 1
}
formats() {
  rest=$1
  while [ -n "$rest" ]; do
    one=${rest%%,*}
    case $one in
      human | short | json | json-diagnostic-short | json-diagnostic-rendered-ansi | json-render-diagnostics) ;;
      *)
        printf 'error: invalid message format specifier: `%s`\n' "$one" >&2
        exit 101 ;;
    esac
    case $rest in *,*) rest=${rest#*,} ;; *) rest= ;; esac
  done
}
check() {
  case ${1-} in
    build | b) shift ;;
    --version | -V)
      printf 'cargo 1.99.1 (stand-in)\n'
      exit 0 ;;
    '' | --help | -h)
      printf 'Rust'"'"'s package manager\n\nUsage: cargo [OPTIONS] [COMMAND]\n'
      exit 0 ;;
    -*) unexpected "$1" ;;
    *)
      printf 'error: no such command: `%s`\n' "$1" >&2
      exit 101 ;;
  esac
  while [ $# -gt 0 ]; do
    case $1 in
      --message-format=*) formats "${1#*=}" ;;
      --message-format)
        if [ $# -lt 2 ]; then
          printf "error: a value is required for '--message-format <FMT>' but none was supplied\n" >&2
          exit 1
        fi
        formats "$2"
        shift ;;
      -r | --release | -q | --quiet | -v | --verbose | --offline | --locked | --frozen | --all-targets | --all-features | --no-default-features | --lib | --bins | --workspace | --keep-going | --future-incompat-report | --timings) ;;
      -p | --package | --bin | -F | --features | --target | --profile | -j | --jobs | --manifest-path | --target-dir | --example | --test | --bench | --config | -Z | --color | --exclude | --artifact-dir)
        if [ $# -lt 2 ]; then
          printf "error: a value is required for '%s' but none was supplied\n" "$1" >&2
          exit 1
        fi
        shift ;;
      --package=* | --bin=* | --features=* | --target=* | --profile=* | --jobs=* | --manifest-path=* | --target-dir=* | --config=* | --color=*) ;;
      *) unexpected "$1" ;;
    esac
    shift
  done
  dir=$PWD
  while [ ! -f "$dir/Cargo.toml" ]; do
    if [ -z "$dir" ] || [ "$dir" = / ]; then
      printf 'error: could not find `Cargo.toml` in `%s` or any parent directory\n' "$PWD" >&2
      exit 101
    fi
    dir=${dir%/*}
  done
}
@CHECKED@
# The test's answer.
@ANSWER@
"#;

/// A stand-in cargo in `bin` whose `cargo build` runs `answer` (sh).
pub fn cargo(bin: &Path, answer: &str) -> PathBuf {
    install_in(
        bin,
        "cargo",
        &CARGO
            .replace("@ANSWER@", answer)
            .replace("@CHECKED@", CHECKED),
    )
}

// ── lazygit ─────────────────────────────────────────────────────────────────

const LAZYGIT: &str = r#"#!/bin/sh
# A stand-in for lazygit (tests/common/stand_in.rs): it refuses an argument
# lazygit doesn't have, as lazygit's flag parser does, says where it runs,
# and waits, as lazygit does, until it is ended.
@PROLOGUE@
takes_value() {
  case $1 in
    -p | --path | -f | --filter | -ucf | --use-config-file | -w | --work-tree | -g | --git-dir | -sm | --screen-mode) return 0 ;;
  esac
  return 1
}
check() {
  prev=
  for arg do
    if takes_value "$prev"; then
      prev=
      continue
    fi
    case $arg in
      -h | --help | -v | --version | -d | --debug | -l | --logs | -cd | --print-config-dir | -sp | --print-default-config) exit 0 ;;
      status | branch | log | stash) ;;
      *)
        if ! takes_value "$arg"; then
          printf 'Unknown arguments supplied:  %s\n' "$arg" >&2
          exit 2
        fi ;;
    esac
    prev=$arg
  done
  if takes_value "$prev"; then
    printf 'Argument missing for flag: %s\n' "$prev" >&2
    exit 2
  fi
}
@CHECKED@
echo "stand-in lazygit in $PWD"
exec sleep 60
"#;

/// A stand-in lazygit in `bin`.
pub fn lazygit(bin: &Path) -> PathBuf {
    install_in(bin, "lazygit", &LAZYGIT.replace("@CHECKED@", CHECKED))
}
