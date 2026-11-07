//! Compiles the skills in `assets/skills/` into the binary, so `switchboard
//! setup skills` can write them out with nothing beside the binary: a table
//! of each file's path under `assets/skills/` and its bytes
//! (`include_bytes!`), written to `$OUT_DIR/skills.rs` for src/setup/skills.rs
//! to include. What setup.sh linked from a clone, the binary now carries.
//!
//! The packages a skill's scripts install for themselves (`npm ci` makes
//! `scripts/node_modules` in a clone that ran them) and hidden files (an
//! editor's or the Finder's) are left out: only what git keeps ships.

use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/skills");
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
