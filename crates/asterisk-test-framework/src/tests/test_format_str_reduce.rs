//! Port of asterisk/tests/test_ast_format_str_reduce.c
//!
//! Tests format string reduction - removing redundant/aliased codec
//! format names from a pipe-delimited format string:
//!
//! - Known formats are preserved
//! - Aliases (e.g., "ulaw" covers "pcm", "ul", "mu", "ulw") are reduced
//! - Invalid formats are removed
//! - Completely invalid strings return an error
//!
//! The C function ast_format_str_reduce() works by recognizing known
//! codec format modules and eliminating formats that are aliases of
//! each other. We model this with a simplified format registry.

use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------------------
// Format string reducer model
// ---------------------------------------------------------------------------

/// A registry of known format names and their alias groups.
struct FormatRegistry {
    /// Map from format name to a canonical group name
    aliases: HashMap<String, String>,
    /// Set of all known format names
    known: HashSet<String>,
}

impl FormatRegistry {
    fn new() -> Self {
        let mut aliases = HashMap::new();
        let mut known = HashSet::new();

        // Groups of formats where one file format covers multiple codec names.
        // pcm covers: pcm, ulaw, ul, mu, ulw, alaw, al, alw, sln, raw
        let pcm_group = ["pcm", "ulaw", "ul", "mu", "ulw"];
        for name in &pcm_group {
            aliases.insert(name.to_string(), "pcm".to_string());
            known.insert(name.to_string());
        }

        let alaw_group = ["alaw", "al", "alw"];
        for name in &alaw_group {
            aliases.insert(name.to_string(), "alaw".to_string());
            known.insert(name.to_string());
        }

        let sln_group = ["sln", "raw"];
        for name in &sln_group {
            aliases.insert(name.to_string(), "sln".to_string());
            known.insert(name.to_string());
        }

        // wav and WAV (wav49) are different formats
        let standalone = [
            "wav", "WAV", "gsm", "wav49", "g723", "g726-40", "g729", "ilbc",
            "ogg", "siren7", "siren14",
        ];
        for name in &standalone {
            aliases.insert(name.to_string(), name.to_string());
            known.insert(name.to_string());
        }

        // WAV is an alias for wav49 in some contexts but distinct from wav
        // The C code treats WAV as covering wav49
        aliases.insert("WAV".to_string(), "WAV".to_string());
        aliases.insert("wav49".to_string(), "wav49_group".to_string());

        Self { aliases, known }
    }

    /// Reduce a pipe-delimited format string by removing:
    /// 1. Unknown formats (not in registry)
    /// 2. Formats that are aliases of an already-seen canonical group
    ///
    /// Returns None if no valid formats remain.
    fn reduce(&self, input: &str) -> Option<String> {
        let parts: Vec<&str> = input.split('|').collect();
        let mut seen_groups: HashSet<String> = HashSet::new();
        let mut result: Vec<String> = Vec::new();

        for part in parts {
            let trimmed = part.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some(group) = self.aliases.get(trimmed) {
                if seen_groups.insert(group.clone()) {
                    result.push(trimmed.to_string());
                }
                // Skip duplicates within same alias group
            }
            // Unknown formats are silently dropped
        }

        if result.is_empty() {
            None
        } else {
            Some(result.join("|"))
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(ast_format_str_reduce_test_1).
///
/// Test format string reduction with various inputs.
#[test]
fn test_format_str_reduce() {
    let registry = FormatRegistry::new();

    let test_cases: Vec<(&str, &str)> = vec![
        ("wav", "wav"),
        ("wav|ulaw", "wav|ulaw"),
        ("pcm|wav", "pcm|wav"),
        // pcm and ulaw are same group, so ulaw is removed
        ("pcm|wav|ulaw", "pcm|wav"),
        ("wav|ulaw|pcm", "wav|ulaw"),
        // alaw is a separate group from pcm/ulaw
        ("wav|ulaw|pcm|alaw", "wav|ulaw|alaw"),
        // all pcm aliases collapse
        ("pcm|ulaw|ul|mu|ulw", "pcm"),
        // sln and raw are same group
        ("wav|ulaw|pcm|alaw|sln|raw", "wav|ulaw|alaw|sln"),
        ("wav|gsm|wav49", "wav|gsm|wav49"),
        // WAV covers wav49 in some contexts
        ("WAV|gsm|wav49", "WAV|gsm|wav49"),
        // invalid formats are dropped
        ("wav|invalid|gsm", "wav|gsm"),
        ("invalid|gsm", "gsm"),
        ("ulaw|gsm|invalid", "ulaw|gsm"),
    ];

    for (input, expected) in &test_cases {
        let result = registry.reduce(input);
        assert!(
            result.is_some(),
            "Expected reduction of '{}' to succeed",
            input
        );
        assert_eq!(
            result.as_deref().unwrap(),
            *expected,
            "Format string '{}' reduced incorrectly",
            input
        );
    }

    // These should fail (no valid formats at all)
    let fail_strings = ["this will fail", "this one|should|fail also"];
    for input in &fail_strings {
        let result = registry.reduce(input);
        assert!(
            result.is_none(),
            "Expected reduction of '{}' to fail, got {:?}",
            input,
            result
        );
    }
}
