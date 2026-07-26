//! Log convention invariant: no `tracing` macro in `src/` may build its
//! message with `serde_json::json!`.
//!
//! The legacy pattern serializes a whole JSON document into the log's
//! `message` field, which turns the data back into text and kills per-field
//! aggregation, which is the entire point of emitting JSON. LUC-130 converted
//! the library modules and left `main.rs` behind without anyone noticing,
//! because nothing was checking. This is that something.

/// Files from `src/` that get scanned. `include_str!` ties the check to the
/// content that actually compiles, so a new file has to be added here on
/// purpose instead of slipping through a glob nobody read.
const SOURCES: &[(&str, &str)] = &[
    ("src/main.rs", include_str!("../src/main.rs")),
    ("src/lib.rs", include_str!("../src/lib.rs")),
];

#[test]
fn no_log_macro_builds_its_message_with_serde_json() {
    let macros = ["info!", "warn!", "error!", "debug!", "trace!"];
    let mut offenders = Vec::new();

    for (path, src) in SOURCES {
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
                offenders.push(format!("{path}:{}: {}", idx + 1, line.trim()));
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
