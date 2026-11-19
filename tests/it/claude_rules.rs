//! `.claude/rules/`: the rules for one part of the code, which Claude Code
//! loads only once a session works on a file a rule's `paths:` names. A
//! rule whose paths name nothing would never load, and one missing from
//! CLAUDE.md's index would go unnoticed, so both are checked here.

use std::path::{Path, PathBuf};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

/// A rule's `paths:` patterns, from its front matter.
fn paths_of(text: &str) -> Vec<String> {
    let front = text
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .map(|(front, _)| front)
        .unwrap_or_else(|| panic!("no front matter"));
    let mut lines = front.lines();
    assert_eq!(lines.next(), Some("paths:"), "front matter is paths: alone");
    lines
        .map(|l| {
            let item = l.trim().strip_prefix("- ").expect("a list item");
            item.trim_matches('"').to_string()
        })
        .collect()
}

/// A glob as a regular expression: `**` any path, `*` any name.
fn regex_of(glob: &str) -> regex::Regex {
    let mut re = String::from("^");
    let mut rest = glob;
    while let Some(c) = rest.chars().next() {
        if let Some(r) = rest.strip_prefix("**") {
            re.push_str(".*");
            rest = r;
        } else if c == '*' {
            re.push_str("[^/]*");
            rest = &rest[1..];
        } else {
            re.push_str(&regex::escape(&c.to_string()));
            rest = &rest[c.len_utf8()..];
        }
    }
    re.push('$');
    regex::Regex::new(&re).unwrap()
}

/// Every file under `dir`, relative to the repository.
fn files_under(dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if !matches!(
                path.file_name().and_then(|n| n.to_str()),
                Some("target" | ".git" | "node_modules")
            ) {
                files_under(&path, out);
            }
        } else if let Ok(rel) = path.strip_prefix(ROOT) {
            out.push(rel.to_string_lossy().into_owned());
        }
    }
}

fn rules() -> Vec<PathBuf> {
    let mut all: Vec<PathBuf> = std::fs::read_dir(Path::new(ROOT).join(".claude/rules"))
        .expect(".claude/rules")
        .flatten()
        .map(|e| e.path())
        .collect();
    all.sort();
    all
}

#[test]
fn every_rule_names_files_that_exist_and_is_in_the_index() {
    let index = std::fs::read_to_string(Path::new(ROOT).join("CLAUDE.md")).unwrap();
    let rules = rules();
    assert!(!rules.is_empty());
    for rule in rules {
        let name = rule.file_name().unwrap().to_str().unwrap().to_string();
        assert!(name.ends_with(".md"), "{name}: a rule is Markdown");
        assert!(
            index.contains(&format!("- `{name}`:")),
            "{name} is in CLAUDE.md's index"
        );
        let text = std::fs::read_to_string(&rule).unwrap();
        let globs = paths_of(&text);
        assert!(!globs.is_empty(), "{name} names its files");
        for glob in globs {
            // Walk only from the part before the first wildcard.
            let fixed: Vec<&str> = glob.split('/').take_while(|s| !s.contains('*')).collect();
            let start = Path::new(ROOT).join(fixed.join("/"));
            let mut files = Vec::new();
            if start.is_file() {
                files.push(glob.clone());
            } else {
                files_under(&start, &mut files);
            }
            let re = regex_of(&glob);
            assert!(
                files.iter().any(|f| re.is_match(f)),
                "{name}: {glob} names no file"
            );
        }
    }
}

#[test]
fn a_glob_reads_as_claude_code_reads_it() {
    assert!(regex_of("src/**").is_match("src/mux/server.rs"));
    assert!(regex_of("crates/*/tests/**").is_match("crates/diff/tests/alone.rs"));
    assert!(!regex_of("crates/*/tests/**").is_match("crates/diff/src/tests/x.rs"));
    assert!(regex_of("Cargo.toml").is_match("Cargo.toml"));
}
