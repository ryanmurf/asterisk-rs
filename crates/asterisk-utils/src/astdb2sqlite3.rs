//! astdb2sqlite3 - Convert Asterisk's AstDB (BerkeleyDB) to SQLite3
//!
//! The original Asterisk used BerkeleyDB for its internal database (AstDB).
//! Modern Asterisk uses SQLite3. This utility reads a BerkeleyDB-format AstDB
//! dump and writes the key/value pairs into a SQLite3 database.
//!
//! Since BerkeleyDB is not commonly available as a Rust crate with good
//! ergonomics, this tool reads from a text dump format (as produced by
//! `db_dump` from BerkeleyDB utilities) and creates the SQLite3 database
//! that Asterisk expects.
//!
//! Port of the original astdb2sqlite3 concept from Asterisk utilities.

use clap::Parser;
use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::process;

/// Convert AstDB (BerkeleyDB text dump) to SQLite3-compatible SQL
#[derive(Parser, Debug)]
#[command(
    name = "astdb2sqlite3",
    about = "Convert AstDB BerkeleyDB text dump to SQLite3 SQL output"
)]
struct Args {
    /// Input file: BerkeleyDB text dump (key/value pairs, one per line as key<TAB>value).
    /// Use '-' for stdin.
    #[arg(short, long)]
    input: PathBuf,

    /// Output SQL file. Use '-' for stdout.
    #[arg(short, long)]
    output: PathBuf,

    /// Table name in the SQLite3 database
    #[arg(short, long, default_value = "astdb")]
    table: String,
}

/// Represents a single AstDB entry with a family/key path and value.
#[derive(Debug, Clone)]
struct AstDbEntry {
    key: String,
    value: String,
}

/// Parse a BerkeleyDB text dump line into key/value.
///
/// Expected format: lines of `key\tvalue` or `/family/key\tvalue`
fn parse_line(line: &str) -> Option<AstDbEntry> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }

    // Split on first tab
    if let Some(tab_pos) = line.find('\t') {
        let key = &line[..tab_pos];
        let value = &line[tab_pos + 1..];
        Some(AstDbEntry {
            key: key.to_string(),
            value: value.to_string(),
        })
    } else {
        // Some dump formats have just the key with empty value
        Some(AstDbEntry {
            key: line.to_string(),
            value: String::new(),
        })
    }
}

/// Escape a string for inclusion in SQL (single-quote doubling).
fn sql_escape(s: &str) -> String {
    s.replace('\'', "''")
}

/// Generate SQL statements for the given entries.
fn generate_sql(table: &str, entries: &[AstDbEntry]) -> String {
    let mut sql = String::new();

    // Create table
    sql.push_str(&format!(
        "CREATE TABLE IF NOT EXISTS \"{table}\" (key VARCHAR(256), value VARCHAR(256));\n"
    ));
    sql.push_str("BEGIN TRANSACTION;\n");

    for entry in entries {
        sql.push_str(&format!(
            "INSERT INTO \"{}\" (key, value) VALUES ('{}', '{}');\n",
            table,
            sql_escape(&entry.key),
            sql_escape(&entry.value),
        ));
    }

    sql.push_str("COMMIT;\n");
    sql
}

fn main() {
    let args = Args::parse();

    // Read input
    let input_text = if args.input.to_string_lossy() == "-" {
        let stdin = io::stdin();
        let mut lines = Vec::new();
        for line in stdin.lock().lines() {
            match line {
                Ok(l) => lines.push(l),
                Err(e) => {
                    eprintln!("Error reading stdin: {e}");
                    process::exit(1);
                }
            }
        }
        lines.join("\n")
    } else {
        match fs::read_to_string(&args.input) {
            Ok(content) => content,
            Err(e) => {
                eprintln!("Error reading {}: {e}", args.input.display());
                process::exit(1);
            }
        }
    };

    // Parse entries
    let entries: Vec<AstDbEntry> = input_text
        .lines()
        .filter_map(parse_line)
        .collect();

    if entries.is_empty() {
        eprintln!("Warning: no entries found in input");
    }

    // Check for duplicate keys
    let mut seen = HashMap::new();
    for entry in &entries {
        *seen.entry(entry.key.clone()).or_insert(0u32) += 1;
    }
    let dups: Vec<_> = seen.iter().filter(|(_, &v)| v > 1).collect();
    if !dups.is_empty() {
        eprintln!(
            "Warning: {} duplicate key(s) found; all will be inserted",
            dups.len()
        );
    }

    // Generate SQL
    let sql = generate_sql(&args.table, &entries);

    // Write output
    if args.output.to_string_lossy() == "-" {
        print!("{sql}");
    } else {
        match fs::File::create(&args.output) {
            Ok(mut f) => {
                if let Err(e) = f.write_all(sql.as_bytes()) {
                    eprintln!("Error writing {}: {e}", args.output.display());
                    process::exit(1);
                }
            }
            Err(e) => {
                eprintln!("Error creating {}: {e}", args.output.display());
                process::exit(1);
            }
        }
    }

    eprintln!(
        "Converted {} entries to SQLite3 SQL (table: {})",
        entries.len(),
        args.table
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_line_with_tab() {
        let entry = parse_line("/family/key\tvalue123").unwrap();
        assert_eq!(entry.key, "/family/key");
        assert_eq!(entry.value, "value123");
    }

    #[test]
    fn test_parse_line_empty() {
        assert!(parse_line("").is_none());
        assert!(parse_line("# comment").is_none());
    }

    #[test]
    fn test_parse_line_no_tab() {
        let entry = parse_line("/family/key").unwrap();
        assert_eq!(entry.key, "/family/key");
        assert_eq!(entry.value, "");
    }

    #[test]
    fn test_sql_escape() {
        assert_eq!(sql_escape("it's"), "it''s");
        assert_eq!(sql_escape("normal"), "normal");
    }

    #[test]
    fn test_generate_sql() {
        let entries = vec![AstDbEntry {
            key: "/test/key".to_string(),
            value: "hello".to_string(),
        }];
        let sql = generate_sql("astdb", &entries);
        assert!(sql.contains("CREATE TABLE"));
        assert!(sql.contains("INSERT INTO"));
        assert!(sql.contains("/test/key"));
        assert!(sql.contains("hello"));
        assert!(sql.contains("BEGIN TRANSACTION"));
        assert!(sql.contains("COMMIT"));
    }
}
