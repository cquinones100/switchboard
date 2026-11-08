//! Compiles the skills in `assets/skills/` into the binary, so `switchboard
//! setup skills` can write them out with nothing beside the binary: a table
//! of each file's path under `assets/skills/` and its bytes
//! (`include_bytes!`), written to `$OUT_DIR/skills.rs` for src/setup/skills.rs
//! to include. What setup.sh linked from a clone, the binary now carries.
//!
//! The packages a skill's scripts install for themselves (`npm ci` makes
//! `scripts/node_modules` in a clone that ran them) and hidden files (an
//! editor's or the Finder's) are left out: only what git keeps ships.
//!
//! It also names the commit the binary is built from, for `switchboard
//! --version` and so a release can be traced to its source (`commit`), and,
//! in a build that isn't a release, the checkout and target directory it is
//! built in, which M-p's restart builds again (`checkout`).

use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let root = manifest_dir().join("assets/skills");
    println!("cargo:rerun-if-changed=assets/skills");
    let mut files = Vec::new();
    walk(&root, &mut files);
    files.sort();
    let mut table = String::from(
        "/// Every file of every skill: its path under `assets/skills/`, and its bytes.\n\
         pub static FILES: &[(&str, &[u8])] = &[\n",
    );
    for file in &files {
        let rel = file
            .strip_prefix(&root)
            .expect("under assets/skills")
            .to_string_lossy()
            .replace('\\', "/");
        table.push_str(&format!(
            "    ({rel:?}, include_bytes!({:?})),\n",
            file.to_string_lossy()
        ));
    }
    table.push_str("];\n");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    fs::write(out.join("skills.rs"), table).expect("the table written");
    commit();
    checkout();
}

/// The checkout and the target directory this build is in, as
/// `SWITCHBOARD_CHECKOUT` and `SWITCHBOARD_TARGET_DIR`, for M-p's "Restart
/// Switchboard on the latest build on disk" (src/mux/latest.rs) to build
/// again; both empty in a release build (`SWITCHBOARD_RELEASE` set), which
/// has no checkout on the computer it runs on. The target directory is the
/// one holding `OUT_DIR` that cargo marks with its `CACHEDIR.TAG`, so a
/// build for another target (`--target`) names it too. With them goes the
/// `cargo` the build's PATH had (rustup's, which reads the checkout's
/// `rust-toolchain.toml`), as `SWITCHBOARD_CARGO`, for a server whose PATH
/// has none.
fn checkout() {
    println!("cargo:rerun-if-env-changed=SWITCHBOARD_RELEASE");
    let release = std::env::var("SWITCHBOARD_RELEASE").is_ok_and(|v| !v.is_empty() && v != "0");
    let (checkout, target, cargo) = if release {
        (String::new(), String::new(), String::new())
    } else {
        let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
        let target = out
            .ancestors()
            .find(|dir| dir.join("CACHEDIR.TAG").is_file())
            .map(|dir| dir.display().to_string())
            .unwrap_or_default();
        let cargo = std::env::var_os("PATH")
            .and_then(|path| {
                std::env::split_paths(&path)
                    .map(|dir| dir.join("cargo"))
                    .find(|cargo| cargo.is_file())
            })
            .map(|cargo| cargo.display().to_string())
            .unwrap_or_default();
        (manifest_dir().display().to_string(), target, cargo)
    };
    println!("cargo:rustc-env=SWITCHBOARD_CHECKOUT={checkout}");
    println!("cargo:rustc-env=SWITCHBOARD_TARGET_DIR={target}");
    println!("cargo:rustc-env=SWITCHBOARD_CARGO={cargo}");
}

/// The checkout being built, read as the build script runs rather than
/// compiled into it (`env!`): a target directory shared by several
/// worktrees reuses one compiled build script for all of them, which would
/// otherwise read the skills and git of whichever worktree compiled it first,
/// gone once that worktree is removed.
fn manifest_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"))
}

/// The commit the binary is built from, for `switchboard --version`, as
/// `SWITCHBOARD_COMMIT`: the variable itself when the build is given one (a
/// build from a source archive, which has no `.git`), else git's short hash,
/// else "unknown" (no git, or no repository). Built again when HEAD moves:
/// git's own files for it are watched, which a worktree keeps apart from its
/// main clone.
fn commit() {
    println!("cargo:rerun-if-env-changed=SWITCHBOARD_COMMIT");
    if let Some(given) = std::env::var("SWITCHBOARD_COMMIT")
        .ok()
        .filter(|c| !c.trim().is_empty())
    {
        println!("cargo:rustc-env=SWITCHBOARD_COMMIT={}", given.trim());
        return;
    }
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(manifest_dir())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    for watched in [
        git(&["rev-parse", "--git-path", "HEAD"]),
        git(&["rev-parse", "--symbolic-full-name", "HEAD"])
            .filter(|r| r.starts_with("refs/"))
            .and_then(|r| git(&["rev-parse", "--git-path", &r])),
        git(&["rev-parse", "--git-path", "packed-refs"]),
    ]
    .into_iter()
    .flatten()
    {
        let path = manifest_dir().join(watched);
        if path.exists() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
    let commit = git(&["rev-parse", "--short=12", "HEAD"])
        .filter(|hash| !hash.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=SWITCHBOARD_COMMIT={commit}");
}

/// Every file under `dir`, but in `node_modules` and hidden ones.
fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        let path = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_dir() => walk(&path, files),
            Ok(t) if t.is_file() => files.push(path),
            _ => {}
        }
    }
}
