//! Small things every part needs: the time, files written whole or kept
//! private, JSON read leniently, commands run with their output, and an async
//! runtime for the parts that wait on several things at once.

use std::ffi::OsStr;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use serde_json::Value;

// ── Time ────────────────────────────────────────────────────────────────────

/// Milliseconds since the epoch, as JavaScript's `Date.now()`.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Whole seconds since the epoch, as `Math.floor(Date.now() / 1000)`.
pub fn now_secs() -> i64 {
    now_ms().div_euclid(1000)
}

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

/// The command's path on `PATH`, if it is there and executable.
pub fn which(command: &str) -> Option<PathBuf> {
    if command.contains('/') {
        let p = PathBuf::from(command);
        return is_executable(&p).then_some(p);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(command))
        .find(|p| is_executable(p))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
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

/// The whole output of a command that must succeed, its stdin closed.
pub fn output_of(cmd: &mut Command) -> Result<Output> {
    let program = cmd.get_program().to_string_lossy().into_owned();
    let output = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                anyhow!("{program}: command not found")
            } else {
                anyhow!("{program}: {e}")
            }
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let first = stderr
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim();
        let how = match output.status.code() {
            Some(code) => format!("exited {code}"),
            None => "was killed".to_string(),
        };
        return Err(anyhow!(
            "{program} {how}{}",
            if first.is_empty() {
                String::new()
            } else {
                format!(": {first}")
            }
        ));
    }
    Ok(output)
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

/// A file's text, or `None` when it can't be read.
pub fn read_text(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path).ok()
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

/// Writes a file whole: to `<file>.<pid>.tmp` beside it, then renamed into
/// place, so a reader never sees half of it. Its directory is made first.
pub fn write_atomic(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> io::Result<()> {
    let path = path.as_ref();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = tmp_beside(path);
    fs::write(&tmp, contents)?;
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

fn tmp_beside(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(format!(".{}.tmp", std::process::id()));
    path.with_file_name(name)
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
        let _ = fs::create_dir_all(dir);
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

/// `JSON.stringify(value)`.
pub fn json_compact(value: &impl serde::Serialize) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".into())
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

/// Standard base64 with padding, for the clipboard sequence (OSC 52) and the
/// few places a protocol wants it.
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_pads_as_the_standard_says() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
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
