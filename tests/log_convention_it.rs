//! Log convention invariant: no `tracing` macro in `src/` may build its
//! message with `serde_json::json!`.
//!
//! The legacy pattern serializes a whole JSON document into the log's
//! `message` field, which turns the data back into text and kills per-field
//! aggregation, which is the entire point of emitting JSON. LUC-130 converted
//! the library modules and left `main.rs` behind without anyone noticing,
//! because nothing was checking. This is that something.

use std::path::{Path, PathBuf};

/// Walks `src/` and returns every `.rs` file, deepest-first order irrelevant.
/// Plain `std::fs` recursion, no crate: the tree is small and a dependency for
/// this would not earn its keep.
fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_log_macro_builds_its_message_with_serde_json() {
    // The whole `src/` tree is scanned at run time instead of an explicit
    // `include_str!` list. This assertion is "zero offenders anywhere", not a
    // count, so pulling in more files can only widen the protection: a module
    // added tomorrow is covered without anyone remembering to register it.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_sources(&root, &mut files);
    files.sort();
    assert!(
        !files.is_empty(),
        "found no .rs file under {}",
        root.display()
    );

    let macros = ["info!", "warn!", "error!", "debug!", "trace!"];
    let mut offenders = Vec::new();

    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        let shown = path
            .strip_prefix(env!("CARGO_MANIFEST_DIR"))
            .unwrap_or(path)
            .display()
            .to_string()
            .replace('\\', "/");
        let lines: Vec<&str> = src.lines().collect();
        for (idx, line) in lines.iter().enumerate() {
            if !macros.iter().any(|m| line.contains(m)) {
                continue;
            }
            // The macro and the `json!` may sit on the same line or on the
            // following ones (rustfmt breaks the call up). Looking at the next
            // two lines covers both shapes.
            let window = lines[idx..(idx + 3).min(lines.len())].join(" ");
            if window.contains("json!") {
                offenders.push(format!("{shown}:{}: {}", idx + 1, line.trim()));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "log macros must use tracing fields, not a hand-built json! message.\n\
         The nested json ends up escaped inside `message` and stops being \
         queryable.\n{}",
        offenders.join("\n")
    );
}
