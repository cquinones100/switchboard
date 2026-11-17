//! Small things every part needs: the time, files written whole or kept
//! private, JSON read leniently, commands run with their output, and an async
//! runtime for the parts that wait on several things at once.

use std::ffi::OsStr;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
#[cfg(test)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde_json::Value;
pub use switchboard_plugin_api::sdk::{
    base64, exe, json_compact, make_dirs, make_private, now_ms, now_secs, output_of, read_text,
    which, write_atomic,
};

// ── Time ────────────────────────────────────────────────────────────────────

/// Now, in the system's time zone (`TZ`, else `/etc/localtime`), as
/// JavaScript's `new Date()` reads its local fields.
pub fn local_now() -> jiff::Zoned {
    jiff::Zoned::now()
}

/// An epoch time in milliseconds, in the system's time zone.
pub fn local_at_ms(ms: i64) -> jiff::Zoned {
    jiff::Timestamp::from_millisecond(ms)
        .unwrap_or(jiff::Timestamp::UNIX_EPOCH)
        .to_zoned(jiff::tz::TimeZone::system())
}

/// `YYYY-MM-DD` of a local time.
pub fn ymd(at: &jiff::Zoned) -> String {
    format!("{:04}-{:02}-{:02}", at.year(), at.month(), at.day())
}

// ── Processes ───────────────────────────────────────────────────────────────

/// Whether a process is running: `kill(pid, 0)`, EPERM counting as running,
/// since it is there but someone else's.
pub fn is_alive(pid: i64) -> bool {
    if pid <= 0 || pid > i32::MAX as i64 {
        return false;
    }
    // SAFETY: signal 0 only checks that the process exists.
    let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
    r == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// A command's stdout, as `execFileSync(command, args, { encoding: "utf8" })`
/// gives it: an error, carrying the first line of stderr, when it can't start
/// or exits non-zero. Its stdin is closed.
pub fn run<I, S>(command: &str, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = Command::new(command);
    cmd.args(args);
    run_command(&mut cmd)
}

/// As `run`, for a command built by the caller (its directory, environment).
pub fn run_command(cmd: &mut Command) -> Result<String> {
    let output = output_of(cmd)?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// A command's stdout, or `None` when it fails in any way.
pub fn try_run<I, S>(command: &str, args: I) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run(command, args).ok()
}

/// A command run with its stdin fed `input`, its stdout returned.
pub fn run_with_input(cmd: &mut Command, input: &[u8]) -> Result<String> {
    let program = cmd.get_program().to_string_lossy().into_owned();
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("{program}: could not start"))?;
    {
        let mut stdin = child.stdin.take().expect("stdin piped");
        // A program that exits without reading all of it is not an error.
        let _ = stdin.write_all(input);
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "{program} failed: {}",
            stderr.lines().next().unwrap_or("").trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Runs an async block to its end on a runtime of its own, for a command that
/// needs async for a moment (a few requests at once). The long-running parts
/// (the sidebar daemon, the Slack relay) build their own.
pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("an async runtime")
        .block_on(future)
}

/// An async command's output, given up on after `timeout`, as
/// `execFile(command, args, { timeout })`: an error when it can't start,
/// times out (the process is killed) or exits non-zero.
pub async fn run_async(
    cmd: &mut tokio::process::Command,
    timeout: Option<Duration>,
) -> Result<Output> {
    let program = cmd.as_std().get_program().to_string_lossy().into_owned();
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = cmd.spawn().map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            anyhow!("{program}: command not found")
        } else {
            anyhow!("{program}: {e}")
        }
    })?;
    let output = match timeout {
        Some(t) => tokio::time::timeout(t, child.wait_with_output())
            .await
            .map_err(|_| anyhow!("{program} timed out"))??,
        None => child.wait_with_output().await?,
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let first = stderr
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim()
            .to_string();
        return Err(anyhow!(
            "{program} failed{}",
            if first.is_empty() {
                String::new()
            } else {
                format!(": {first}")
            }
        ));
    }
    Ok(output)
}

// ── Files ───────────────────────────────────────────────────────────────────

/// The directory itself when it exists, else its nearest ancestor that
/// does: for a directory removed under a program still in it. A `stat` or
/// so, never a process. `None` for a relative path that runs out.
pub fn nearest_existing(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .find(|d| !d.as_os_str().is_empty() && d.is_dir())
        .map(Path::to_path_buf)
}

/// A file read as JSON, or `None` when it is missing or isn't JSON.
pub fn read_json(path: impl AsRef<Path>) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// A file read as a JSON object, or an empty one, as the Node parts read
/// their state files: `try { JSON.parse(...) } catch { return {} }`.
pub fn read_json_object(path: impl AsRef<Path>) -> serde_json::Map<String, Value> {
    match read_json(path) {
        Some(Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    }
}

/// Writes (or, with `append`, appends to) a file only its owner can read,
/// whatever the umask, as `fs.writeFileSync(file, text, { mode: 0o600 })`.
pub fn write_private(
    path: impl AsRef<Path>,
    contents: impl AsRef<[u8]>,
    append: bool,
) -> io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.create(true).mode(0o600);
    if append {
        options.append(true);
    } else {
        options.write(true).truncate(true);
    }
    let mut file = options.open(path)?;
    file.write_all(contents.as_ref())
}

/// Appends a line to a file, making its directory first, ignoring failure:
/// for the nudge files other parts watch.
pub fn append_quietly(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) {
    let path = path.as_ref();
    if let Some(dir) = path.parent() {
        let _ = make_dirs(dir);
    }
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(contents.as_ref());
    }
}

// ── JSON as JavaScript writes it ────────────────────────────────────────────

/// A number as JavaScript would write it: a whole number with no `.0`, so
/// files written here read as they did when Node wrote them.
pub fn js_number(n: f64) -> Value {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 9.007_199_254_740_992e15 {
        Value::from(n as i64)
    } else if n.is_finite() {
        Value::from(n)
    } else {
        // JSON.stringify writes NaN and Infinity as null.
        Value::Null
    }
}

/// `JSON.stringify(value, null, 2)`.
pub fn json_pretty(value: &impl serde::Serialize) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| "null".into())
}

/// Whether a JSON value is truthy as JavaScript sees it.
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// A JSON value as JavaScript's `String(value)` writes it, for the values
/// these files hold: a string as it is, a number or boolean as written,
/// null as "null".
pub fn js_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        Value::Number(n) => n
            .as_f64()
            .map(|f| {
                if f.fract() == 0.0 && f.abs() < 1e21 {
                    format!("{}", f as i64)
                } else {
                    n.to_string()
                }
            })
            .unwrap_or_else(|| n.to_string()),
        other => other.to_string(),
    }
}

/// Standard base64 read back, its padding optional; `None` for anything
/// else (a character outside the alphabet, a length no encoding gives).
pub fn unbase64(text: &[u8]) -> Option<Vec<u8>> {
    let digit = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let body = text.strip_suffix(b"==").or_else(|| text.strip_suffix(b"="));
    let body = body.unwrap_or(text);
    if body.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(body.len() / 4 * 3 + 2);
    for chunk in body.chunks(4) {
        let mut n = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            n |= digit(*c)? << (18 - 6 * i);
        }
        out.extend_from_slice(&n.to_be_bytes()[1..chunk.len()]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode_of(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn folders_made_for_a_file_are_the_owners_alone() {
        let top = std::env::temp_dir().join(format!("sbutil-private-{}", std::process::id()));
        let _ = fs::remove_dir_all(&top);
        write_atomic(top.join("state/switchboard/file.json"), "{}").unwrap();
        append_quietly(top.join("cache/switchboard/nudge"), "x\n");
        for dir in [
            "",
            "state",
            "state/switchboard",
            "cache",
            "cache/switchboard",
        ] {
            assert_eq!(mode_of(&top.join(dir)), 0o700, "{dir}");
        }
        let _ = fs::remove_dir_all(&top);
    }

    #[test]
    fn make_private_makes_and_tightens_your_folders_and_passes_over_links() {
        let top = std::env::temp_dir().join(format!("sbutil-tighten-{}", std::process::id()));
        let _ = fs::remove_dir_all(&top);
        let (open, target) = (top.join("open"), top.join("target"));
        fs::create_dir_all(&open).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::set_permissions(&open, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink(&target, top.join("link")).unwrap();
        make_private(&[open.clone(), top.join("link"), top.join("missing")]);
        assert_eq!(mode_of(&open), 0o700);
        assert_eq!(mode_of(&top.join("missing")), 0o700, "a missing one made");
        assert_eq!(mode_of(&target), 0o755, "a link's folder is left");
        let _ = fs::remove_dir_all(&top);
    }

    #[test]
    fn nearest_existing_climbs_out_of_a_removed_directory() {
        let kept = std::env::temp_dir().join(format!("sbutil-nearest-{}", std::process::id()));
        fs::create_dir_all(&kept).unwrap();
        assert_eq!(nearest_existing(&kept), Some(kept.clone()));
        let removed = kept.join("worktree/sub");
        assert_eq!(nearest_existing(&removed), Some(kept.clone()));
        let _ = fs::remove_dir_all(&kept);
    }

    #[test]
    fn base64_pads_as_the_standard_says() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64_reads_back_with_or_without_its_padding() {
        for text in [&b""[..], b"f", b"fo", b"foo", b"foobar", b"/tmp/a b.png"] {
            assert_eq!(
                unbase64(base64(text).as_bytes()).as_deref(),
                Some(text),
                "{text:?}"
            );
        }
        assert_eq!(unbase64(b"Zm8").as_deref(), Some(&b"fo"[..]));
        assert_eq!(unbase64(b"Zm9v!"), None, "not the alphabet");
        assert_eq!(unbase64(b"Zm9vY"), None, "no encoding is that long");
    }

    #[test]
    fn js_numbers_drop_their_point() {
        assert_eq!(js_number(3.0).to_string(), "3");
        assert_eq!(js_number(1.5).to_string(), "1.5");
        assert_eq!(js_number(f64::NAN), Value::Null);
    }

    #[test]
    fn truthiness_follows_javascript() {
        assert!(!truthy(&Value::from("")));
        assert!(!truthy(&Value::from(0)));
        assert!(truthy(&serde_json::json!([])));
    }

    #[test]
    fn a_process_that_runs_is_alive() {
        assert!(is_alive(std::process::id() as i64));
        assert!(!is_alive(0));
    }
}
