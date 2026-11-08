//! Putting a downloaded release in the place of the running binary, as
//! safely as setup.sh's pinned downloads were: the archive is checked
//! against the SHA-256 published beside it before anything is unpacked
//! (setup.sh's `font_sha256` and `mermaid_ascii` checks), the binary in it
//! is written beside the old one, as `switchboard` in a folder of its own,
//! and run once (`--version`) to show it starts on this computer, and only
//! then renamed over the old one, which
//! is a single step: whatever fails before it leaves the old binary exactly
//! as it was, and nothing half-written is ever in its place.
//!
//! The running binary goes on running through the rename (the file it
//! started from stays open until it ends), so `switchboard update` can
//! replace itself, and so can a server that keeps running the old code
//! until it restarts.

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use sha2::{Digest, Sha256};

/// A file's SHA-256, in lower-case hex, as `shasum -a 256` prints it.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The hash a `.sha256` file gives for `name`: its first word when that is
/// 64 hex digits, as `shasum -a 256 <name>` writes it (`<hash>  <name>`),
/// and the name after it, when there is one, must be the archive's own, so
/// a checksum published for another file is never taken for this one's.
pub fn expected_hash(checksum_file: &str, name: &str) -> Result<String> {
    let line = checksum_file
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .ok_or_else(|| anyhow!("{name}.sha256 is empty"))?;
    let mut words = line.split_whitespace();
    let hash = words.next().unwrap_or("").to_ascii_lowercase();
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("{name}.sha256 doesn't hold a SHA-256");
    }
    if let Some(named) = words.next() {
        // shasum marks a file read as binary with a `*` before its name.
        let named = named.trim_start_matches('*');
        if Path::new(named).file_name().and_then(|n| n.to_str()) != Some(name) {
            bail!("{name}.sha256 is the checksum of {named}, not of {name}");
        }
    }
    Ok(hash)
}

/// Refuses an archive whose SHA-256 isn't the one published beside it.
pub fn verify(archive: &[u8], checksum_file: &str, name: &str) -> Result<()> {
    let expected = expected_hash(checksum_file, name)?;
    let actual = sha256_hex(archive);
    if actual != expected {
        bail!(
            "{name} isn't the file that was published: its SHA-256 is {actual}, and the release says {expected}. Nothing was changed."
        );
    }
    Ok(())
}

/// The `switchboard` binary inside a release's archive, wherever the
/// archive keeps it (the workflow puts it in a folder named after the
/// archive); only a plain file is taken.
pub fn binary_from(archive: &[u8]) -> Result<Vec<u8>> {
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    for entry in tar.entries().context("the archive can't be read")? {
        let mut entry = entry.context("the archive can't be read")?;
        if entry.header().entry_type() != tar::EntryType::Regular {
            continue;
        }
        let is_binary = entry
            .path()
            .ok()
            .and_then(|p| p.file_name().map(|n| n == "switchboard"))
            .unwrap_or(false);
        if is_binary {
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .context("the archive can't be read")?;
            return Ok(bytes);
        }
    }
    bail!("the archive has no switchboard in it")
}

/// How long the new binary may take to say its version.
const RUN_TIMEOUT: Duration = Duration::from_secs(20);

/// Puts `binary` in the place of the file at `target`, as the module's
/// comment says: written beside it, run once, then renamed over it. Gives
/// what the new binary said of its version.
pub fn replace(target: &Path, binary: &[u8]) -> Result<String> {
    let dir = target
        .parent()
        .ok_or_else(|| anyhow!("{} has no folder", target.display()))?;
    let name = target
        .file_name()
        .ok_or_else(|| anyhow!("{} has no name", target.display()))?
        .to_string_lossy();
    // A folder of its own beside the old binary, on the same disk so the
    // rename is one step, holding it as `switchboard`: the binary takes a
    // name that isn't a command's as `switchboard`, but a release from
    // before it did read `.switchboard.new-<pid>` as a command it lacks.
    let folder = dir.join(format!(".{name}.new-{}", std::process::id()));
    let new = folder.join("switchboard");
    let result = fs::create_dir(&folder)
        .with_context(|| format!("couldn't write in {}", dir.display()))
        .and_then(|()| write_and_check(&new, binary))
        .and_then(|said| {
            fs::rename(&new, target)
                .with_context(|| format!("couldn't put it in place of {}", target.display()))?;
            Ok(said)
        });
    let _ = fs::remove_file(&new);
    let _ = fs::remove_dir(&folder);
    result
}

fn write_and_check(new: &Path, binary: &[u8]) -> Result<String> {
    let dir = new.parent().unwrap_or(Path::new("."));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o755)
        .open(new)
        .with_context(|| format!("couldn't write in {}", dir.display()))?;
    file.write_all(binary)
        .and_then(|()| file.sync_all())
        .with_context(|| format!("couldn't write in {}", dir.display()))?;
    drop(file);
    fs::set_permissions(new, fs::Permissions::from_mode(0o755))?;
    says_version(new)
}

/// Runs a binary with `--version` and gives the version it says, or why it
/// won't do: one that can't start here (built for another computer), fails,
/// or says something other than Switchboard's version.
pub fn says_version(binary: &Path) -> Result<String> {
    let mut child = spawn_version(binary)?;
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > RUN_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            bail!("the new switchboard didn't say its version");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut out);
    }
    let said = out.lines().next().unwrap_or("").trim().to_string();
    match said.strip_prefix("switchboard ") {
        Some(version) if status.success() => Ok(version.to_string()),
        _ => bail!("the new switchboard didn't say its version"),
    }
}

// Starting a file just written can meet "text file busy" for a moment on
// Linux, while a process forked elsewhere in this one still holds it open
// for writing until it execs; it is tried again for a little while then.
fn spawn_version(binary: &Path) -> Result<std::process::Child> {
    let started = Instant::now();
    loop {
        let spawned = Command::new(binary)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        match spawned {
            Err(e)
                if e.raw_os_error() == Some(libc::ETXTBSY)
                    && started.elapsed() < Duration::from_secs(2) =>
            {
                std::thread::sleep(Duration::from_millis(20));
            }
            other => return other.context("the new switchboard won't start on this computer"),
        }
    }
}

/// Where the binary to replace is: the running one's file, its links
/// followed, so a link to it in `~/.local/bin` stays a link and the file it
/// points at is the one replaced.
pub fn installed_binary() -> Result<PathBuf> {
    let exe = crate::util::exe().context("couldn't find the running switchboard")?;
    Ok(fs::canonicalize(&exe).unwrap_or(exe))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        {
            let mut tar = tar::Builder::new(&mut gz);
            for (path, bytes) in files {
                let mut header = tar::Header::new_gnu();
                header.set_size(bytes.len() as u64);
                header.set_mode(0o755);
                header.set_cksum();
                tar.append_data(&mut header, path, *bytes).unwrap();
            }
            tar.finish().unwrap();
        }
        gz.finish().unwrap()
    }

    #[test]
    fn the_checksum_must_match_and_name_the_archive() {
        let archive = b"the archive";
        let hash = sha256_hex(archive);
        let name = "switchboard-0.2.0-x86_64-apple-darwin.tar.gz";
        assert!(verify(archive, &format!("{hash}  {name}\n"), name).is_ok());
        assert!(verify(archive, &format!("{hash} *{name}\n"), name).is_ok());
        assert!(verify(archive, &format!("{}\n", hash.to_uppercase()), name).is_ok());
        let bad = verify(b"another archive", &format!("{hash}  {name}\n"), name);
        assert!(bad.unwrap_err().to_string().contains("isn't the file"));
        assert!(verify(archive, &format!("{hash}  other.tar.gz\n"), name).is_err());
        assert!(verify(archive, "", name).is_err());
        assert!(verify(archive, "not-a-hash  x", name).is_err());
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn the_binary_is_found_in_its_folder() {
        let archive = archive_of(&[
            ("switchboard-0.2.0-x/README", b"read me"),
            ("switchboard-0.2.0-x/switchboard", b"the binary"),
        ]);
        assert_eq!(binary_from(&archive).unwrap(), b"the binary");
        let empty = archive_of(&[("switchboard-0.2.0-x/README", b"read me")]);
        assert!(binary_from(&empty).is_err());
        assert!(binary_from(b"not gzip at all").is_err());
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sb-install-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    const OLD: &[u8] = b"#!/bin/sh\necho 'switchboard 0.1.0 (old)'\n";

    /// A stand-in release that, as switchboard 0.2.0 did, says its version
    /// only when its file is named `switchboard`, reading any other name as
    /// a command it doesn't have (tests/common/stand_in.rs).
    fn release(version: &str) -> Vec<u8> {
        crate::stand_in::Release::new(version).script().into_bytes()
    }

    #[test]
    fn a_binary_that_runs_takes_the_old_ones_place() {
        let dir = temp("ok");
        let target = dir.join("switchboard");
        fs::write(&target, OLD).unwrap();
        let said = replace(&target, &release("0.2.0 (abc)")).unwrap();
        assert_eq!(said, "0.2.0 (abc)");
        assert!(fs::read_to_string(&target).unwrap().contains("0.2.0"));
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o755
        );
        // Nothing is left beside it.
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_binary_that_wont_run_leaves_the_old_one() {
        let dir = temp("bad");
        let target = dir.join("switchboard");
        fs::write(&target, OLD).unwrap();
        // Broken on purpose: not a program, one that fails, and one that
        // doesn't say it is switchboard.
        for broken in [
            &b"\x7fELF not really a binary"[..],
            b"#!/bin/sh\nexit 3\n",
            b"#!/bin/sh\necho 'something else'\n",
        ] {
            assert!(replace(&target, broken).is_err());
            assert_eq!(fs::read(&target).unwrap(), OLD);
            assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_it_cant_write_in_leaves_the_old_one() {
        let dir = temp("ro");
        let target = dir.join("switchboard");
        fs::write(&target, OLD).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
        // Root writes anywhere, so there is nothing to show as root.
        let writable = fs::write(dir.join("probe"), b"").is_ok();
        if !writable {
            let err = replace(&target, &release("0.2.0")).unwrap_err();
            assert!(err.to_string().contains("couldn't write"));
            assert_eq!(fs::read(&target).unwrap(), OLD);
        }
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }
}
