//! Putting a downloaded release in the place of the running binary, as
//! safely as setup.sh's pinned downloads were: the archive is checked
//! against the SHA-256 published beside it before anything is unpacked
//! (setup.sh's `font_sha256` and `mermaid_ascii` checks), and against its
//! signature, which only the release key makes: the checksum comes from the
//! same release as the archive, so someone able to change one could change
//! the other, while the key's public half is compiled into this binary
//! (`release-signing-keys.pub`), install.sh carrying a copy. The signature
//! is OpenSSH's (`ssh-keygen -Y sign`, namespace `switchboard-release`),
//! which install.sh checks with the `ssh-keygen` every Mac and Linux has.
//! The binary in it
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
//!
//! The checks and the swap are the plugin API's `release`, shared with the
//! diff's own update; these are Switchboard's, by its binary's name.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub use switchboard_plugin_api::release::{
    RELEASE_KEYS, SIGNATURE_NAMESPACE, expected_hash, release_keys, sha256_hex, verify,
    verify_signature,
};
use switchboard_plugin_api::release::{SWITCHBOARD, binary_named, replace_named, says_version_of};

/// The `switchboard` binary inside a release's archive, wherever the
/// archive keeps it (the workflow puts it in a folder named after the
/// archive); only a plain file is taken.
pub fn binary_from(archive: &[u8]) -> Result<Vec<u8>> {
    binary_named(archive, SWITCHBOARD)
}

/// Puts `binary` in the place of the file at `target`, as the module's
/// comment says: written beside it, run once, then renamed over it. Gives
/// what the new binary said of its version.
pub fn replace(target: &Path, binary: &[u8]) -> Result<String> {
    replace_named(target, binary, SWITCHBOARD)
}

/// Runs a binary with `--version` and gives the version it says, or why it
/// won't do: one that can't start here (built for another computer), fails,
/// or says something other than Switchboard's version.
pub fn says_version(binary: &Path) -> Result<String> {
    says_version_of(binary, SWITCHBOARD)
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
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn only_the_release_keys_signature_counts() {
        use ssh_key::{HashAlg, LineEnding, PrivateKey, private::Ed25519Keypair};
        let other = PrivateKey::from(Ed25519Keypair::from_seed(&[7; 32]));
        let name = "switchboard-0.2.0-x86_64-apple-darwin.tar.gz";
        // Made with the release key by `ssh-keygen -Y sign`, as the release
        // workflow signs, over the text it signed.
        let archive = b"Signed with the release key, for its test.\n";
        let by_key = include_str!("release-signing-check.sig");
        verify_signature(archive, by_key, name).unwrap();
        assert!(verify_signature(b"another archive", by_key, name).is_err());

        let by_other = other
            .sign(SIGNATURE_NAMESPACE, HashAlg::Sha512, archive)
            .unwrap()
            .to_pem(LineEnding::LF)
            .unwrap();
        let bad = verify_signature(archive, &by_other, name).unwrap_err();
        assert!(bad.to_string().contains("isn't signed by"), "{bad}");
        assert!(verify_signature(archive, "", name).is_err());
        assert!(verify_signature(archive, "not a signature", name).is_err());
        assert!(!release_keys().unwrap().is_empty());
    }

    #[test]
    fn install_sh_carries_the_same_release_keys() {
        let script = include_str!("../../install.sh");
        for key in RELEASE_KEYS.lines().filter(|l| !l.trim().is_empty()) {
            assert!(script.contains(key), "install.sh lacks {key}");
        }
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
