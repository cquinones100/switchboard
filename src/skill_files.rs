//! Which files under `assets/skills/` are a skill's, for build.rs, which
//! compiles them into the binary (it includes this file by its path, so it
//! uses nothing but std), and for src/assets.rs, which reads them from the
//! clone the build came from: one walk, so the two never disagree on what
//! ships. The packages a skill's scripts install for themselves (`npm ci`
//! makes `scripts/node_modules` in a clone that ran them) and hidden files
//! (an editor's or the Finder's) are left out: only what git keeps ships.

use std::fs;
use std::path::{Path, PathBuf};

/// Every file under `dir`, but in `node_modules` and hidden ones, sorted.
pub fn under(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    walk(dir, &mut files);
    files.sort();
    files
}

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

/// A file's path under `root`, with `/` between its parts.
pub fn relative(root: &Path, file: &Path) -> Option<String> {
    file.strip_prefix(root)
        .ok()
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
}
