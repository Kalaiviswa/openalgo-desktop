//! No deferred transactions on the app's SQLite databases.
//!
//! A deferred transaction that reads and then writes takes the write lock
//! at its first write, and SQLite refuses that upgrade with "database is
//! locked" at once, without calling the busy handler, whenever another
//! connection is writing. A scalping stop save failed that way beside
//! ordinary order traffic (`tests/it/perf/sqlite_wait.rs`). Every writing
//! transaction opens with `write_transaction` (or `BEGIN IMMEDIATE`); this
//! scan fails on a deferred one anywhere in production code, outside the
//! allowlist below.

use std::path::{Path, PathBuf};

/// Files allowed deferred transactions: how many, and why each is safe.
const ALLOWED: &[(&str, usize, &str)] = &[
    (
        "services/master_contract_service.rs",
        2,
        "read-only: the master's owner and its rows are read in one snapshot, nothing is written",
    ),
    (
        "db/duckdb/mod.rs",
        1,
        "DuckDB, not SQLite: no busy handler; its writers take the in-process write lock first",
    ),
    (
        "db/duckdb/migrations.rs",
        1,
        "DuckDB migrations, run while the store is opened, before anything else uses it",
    ),
];

/// The ways a deferred transaction is opened.
fn deferred_openings() -> Vec<String> {
    let begin = "execute_batch(\"BEGIN";
    vec![
        format!(".{}()", "transaction"),
        format!(".{}()", "unchecked_transaction"),
        format!("{}\")", begin),
        format!("{} TRANSACTION\")", begin),
        format!("{} DEFERRED", begin),
        format!("TransactionBehavior::{}", "Deferred"),
    ]
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == "tests") {
                continue;
            }
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name != "tests.rs" && !name.ends_with("_tests.rs") {
                out.push(p);
            }
        }
    }
}

/// Production lines of a file: up to its `#[cfg(test)] mod` block.
fn production(text: &str) -> Vec<&str> {
    let lines: Vec<&str> = text.lines().collect();
    let end = lines
        .windows(2)
        .position(|w| w[0].trim() == "#[cfg(test)]" && w[1].trim_start().starts_with("mod "))
        .unwrap_or(lines.len());
    lines[..end].to_vec()
}

#[test]
fn writing_transactions_open_immediate() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    assert!(files.len() > 100, "the source tree was not found");
    let patterns = deferred_openings();
    let mut problems = Vec::new();
    for f in files {
        let rel = f
            .strip_prefix(&src)
            .unwrap_or(&f)
            .to_string_lossy()
            .replace('\\', "/");
        if rel == "db/sqlite/transaction_guard.rs" {
            continue;
        }
        let text = std::fs::read_to_string(&f).unwrap_or_default();
        let found: Vec<String> = production(&text)
            .iter()
            .enumerate()
            .filter(|(_, l)| patterns.iter().any(|p| l.contains(p.as_str())))
            .map(|(i, l)| format!("{}:{}: {}", rel, i + 1, l.trim()))
            .collect();
        let allowed = ALLOWED
            .iter()
            .find(|(path, ..)| *path == rel)
            .map(|(_, n, _)| *n)
            .unwrap_or(0);
        if found.len() != allowed {
            problems.push(format!(
                "{} deferred transaction(s), {} allowed:\n  {}",
                found.len(),
                allowed,
                found.join("\n  ")
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "Open writing transactions with db::sqlite::write_transaction (BEGIN IMMEDIATE); \
         a read-only one goes in the allowlist with its reason.\n{}",
        problems.join("\n")
    );
}
