//! Every link Switchboard opens says where it is going while the browser
//! comes up (src/links.rs): the server's through `Server::open_link`, the
//! notes app's and the pager's on their message lines, mission control's
//! under its busy box. So nothing else opens one with the plain calls.

use std::fs;
use std::path::{Path, PathBuf};

/// The files that may open a link without `Server::open_link`, since they
/// say the words themselves or are where it is done.
const ALLOWED: &[&str] = &[
    "src/links.rs",
    "src/views/serve.rs",
    "src/notes/app.rs",
    "src/views/pager.rs",
];

/// The calls that open a link with nothing said.
const PLAIN: &[&str] = &["links::open_url", "open_url_as", "open_in_background"];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Whether `text` names `call` as a whole name, not the start of a longer
/// one (`links::open_url_watched`).
fn names(text: &str, call: &str) -> bool {
    text.match_indices(call).any(|(at, _)| {
        !text[at + call.len()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
    })
}

#[test]
fn only_the_places_that_say_so_open_a_link() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    let offending: Vec<String> = files
        .iter()
        .filter_map(|f| {
            let name = f.strip_prefix(root).unwrap().to_string_lossy().into_owned();
            if ALLOWED.contains(&name.as_str()) {
                return None;
            }
            let text = fs::read_to_string(f).unwrap();
            PLAIN
                .iter()
                .find(|call| names(&text, call))
                .map(|call| format!("{name} calls {call}; open a link through Server::open_link"))
        })
        .collect();
    assert!(offending.is_empty(), "{}", offending.join("\n"));
}
