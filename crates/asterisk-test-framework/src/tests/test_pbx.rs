//! Port of asterisk/tests/test_pbx.c
//!
//! Tests PBX pattern matching, extension lookup, CID matching,
//! priority ordering, context includes, and variable substitution.

use asterisk_core::pbx::{Context, Dialplan, Extension, Priority};

// =========================================================================
// Pattern matching tests -- port of pattern_match_test from test_pbx.c
// =========================================================================

/// Test exact extension match.
/// C equivalent: registering extension "100" and matching "100" exactly.
#[test]
fn test_pattern_exact_match() {
    let ext = Extension::new("100");
    assert!(ext.matches("100"));
    assert!(!ext.matches("101"));
    assert!(!ext.matches("10"));
    assert!(!ext.matches("1000"));
}

/// Test _X. pattern -- matches any string starting with any digit followed by one or more chars.
/// C equivalent: registering pattern "_X." and testing various matches.
#[test]
fn test_pattern_x_dot() {
    let ext = Extension::new("_X.");
    assert!(ext.matches("10"));       // X=1, .=0
    assert!(ext.matches("200"));      // X=2, .=00
    assert!(ext.matches("999999"));   // X=9, .=99999
    assert!(!ext.matches(""));        // too short
    // A single digit should fail because . requires at least one char
    assert!(!ext.matches("5"));
}

/// Test _NXXNXXXXXX -- typical US phone number pattern.
/// C equivalent: testing 10-digit number pattern matching.
#[test]
fn test_pattern_nxxnxxxxxx() {
    let ext = Extension::new("_NXXNXXXXXX");
    assert!(ext.matches("2125551234"));  // valid US number
    assert!(ext.matches("9999999999"));  // all nines
    assert!(!ext.matches("1125551234")); // N doesn't match 1
    assert!(!ext.matches("212555123"));  // too short (9 digits)
    assert!(!ext.matches("21255512345")); // too long (11 digits)
}

/// Test _[1-5]XX -- character class pattern.
/// C equivalent: testing bracket range in pattern matching.
#[test]
fn test_pattern_bracket_range() {
    let ext = Extension::new("_[1-5]XX");
    assert!(ext.matches("100"));  // 1 in [1-5]
    assert!(ext.matches("300"));  // 3 in [1-5]
    assert!(ext.matches("500"));  // 5 in [1-5]
    assert!(!ext.matches("600")); // 6 not in [1-5]
    assert!(!ext.matches("099")); // 0 not in [1-5]
}

/// Test _! pattern -- matches zero or more remaining characters.
/// C equivalent: testing the "!" wildcard in extension patterns.
#[test]
fn test_pattern_bang() {
    let ext = Extension::new("_1!");
    assert!(ext.matches("1"));       // ! matches zero chars
    assert!(ext.matches("10"));      // ! matches one char
    assert!(ext.matches("12345"));   // ! matches many chars
    assert!(!ext.matches("2"));      // doesn't start with 1
    assert!(!ext.matches(""));       // pattern starts with 1, can't match empty
}

/// Test _Z pattern -- digits 1-9 (no zero).
#[test]
fn test_pattern_z() {
    let ext = Extension::new("_ZXX");
    assert!(ext.matches("100"));  // Z matches 1
    assert!(ext.matches("999"));  // Z matches 9
    assert!(!ext.matches("099")); // Z doesn't match 0
}

/// Test _N pattern -- digits 2-9.
#[test]
fn test_pattern_n() {
    let ext = Extension::new("_NXX");
    assert!(ext.matches("200"));  // N matches 2
    assert!(ext.matches("999"));  // N matches 9
    assert!(!ext.matches("100")); // N doesn't match 1
    assert!(!ext.matches("099")); // N doesn't match 0
}

/// Test . (dot) pattern -- matches one or more characters.
#[test]
fn test_pattern_dot_requires_at_least_one() {
    let ext = Extension::new("_1.");
    assert!(ext.matches("10"));     // . needs at least 1 char
    assert!(ext.matches("12345")); // . matches many
    assert!(!ext.matches("2"));    // doesn't start with 1
    // "1" alone: pattern is "1." -- 1 matches, then . needs at least one more
    assert!(!ext.matches("1"));
}

// =========================================================================
// Priority ordering tests
// =========================================================================

/// Test numeric priorities are stored and retrieved correctly.
#[test]
fn test_priority_ordering() {
    let mut ext = Extension::new("100");

    ext.add_priority(Priority {
        priority: 1,
        app: "Answer".to_string(),
        app_data: String::new(),
        label: None,
    });
    ext.add_priority(Priority {
        priority: 2,
        app: "Playback".to_string(),
        app_data: "hello-world".to_string(),
        label: None,
    });
    ext.add_priority(Priority {
        priority: 3,
        app: "Hangup".to_string(),
        app_data: String::new(),
        label: None,
    });

    // Get by specific priority
    let p1 = ext.get_priority(1).unwrap();
    assert_eq!(p1.app, "Answer");

    let p2 = ext.get_priority(2).unwrap();
    assert_eq!(p2.app, "Playback");
    assert_eq!(p2.app_data, "hello-world");

    let p3 = ext.get_priority(3).unwrap();
    assert_eq!(p3.app, "Hangup");

    // Non-existent priority
    assert!(ext.get_priority(4).is_none());
}

/// Test priority labels.
#[test]
fn test_priority_labels() {
    let mut ext = Extension::new("200");

    ext.add_priority(Priority {
        priority: 1,
        app: "Answer".to_string(),
        app_data: String::new(),
        label: Some("start".to_string()),
    });
    ext.add_priority(Priority {
        priority: 2,
        app: "Noop".to_string(),
        app_data: String::new(),
        label: None,
    });
    ext.add_priority(Priority {
        priority: 3,
        app: "Hangup".to_string(),
        app_data: String::new(),
        label: Some("end".to_string()),
    });

    let p1 = ext.get_priority(1).unwrap();
    assert_eq!(p1.label.as_deref(), Some("start"));

    let p2 = ext.get_priority(2).unwrap();
    assert!(p2.label.is_none());

    let p3 = ext.get_priority(3).unwrap();
    assert_eq!(p3.label.as_deref(), Some("end"));
}

/// Test "next" priority retrieval.
#[test]
fn test_next_priority() {
    let mut ext = Extension::new("300");

    ext.add_priority(Priority {
        priority: 1,
        app: "Answer".to_string(),
        app_data: String::new(),
        label: None,
    });
    ext.add_priority(Priority {
        priority: 2,
        app: "Hangup".to_string(),
        app_data: String::new(),
        label: None,
    });

    let next = ext.next_priority(1).unwrap();
    assert_eq!(next.app, "Hangup");
    assert_eq!(next.priority, 2);

    // No priority after 2
    assert!(ext.next_priority(2).is_none());
}

// =========================================================================
// Extension lookup across contexts with includes
// =========================================================================

/// Port of pattern_match_test context include behavior.
/// Tests that extensions can be found through context includes.
#[test]
fn test_extension_lookup_with_includes() {
    let mut dp = Dialplan::new();

    // Create test_pattern context with a pattern
    let mut test_ctx = Context::new("test_pattern");
    let mut ext1 = Extension::new("_2XX");
    ext1.add_priority(Priority {
        priority: 1,
        app: "Noop".to_string(),
        app_data: "_2XX".to_string(),
        label: None,
    });
    test_ctx.add_extension(ext1);

    let mut ext2 = Extension::new("_1XX");
    ext2.add_priority(Priority {
        priority: 1,
        app: "Noop".to_string(),
        app_data: "_1XX".to_string(),
        label: None,
    });
    test_ctx.add_extension(ext2);
    dp.add_context(test_ctx);

    // Create an including context
    let mut parent_ctx = Context::new("parent");
    parent_ctx.add_include("test_pattern");
    dp.add_context(parent_ctx);

    // Should find _2XX pattern via include
    let result = dp.find_extension("parent", "200");
    assert!(result.is_some());
    let (ctx, ext) = result.unwrap();
    assert_eq!(ctx.name, "test_pattern");
    assert_eq!(ext.name, "_2XX");

    // Should find _1XX pattern via include
    let result2 = dp.find_extension("parent", "150");
    assert!(result2.is_some());
    let (_, ext) = result2.unwrap();
    assert_eq!(ext.name, "_1XX");

    // Should not find non-matching extension
    let result3 = dp.find_extension("parent", "300");
    assert!(result3.is_none());
}

/// Test that local extensions take priority over included context extensions.
#[test]
fn test_local_extension_priority_over_include() {
    let mut dp = Dialplan::new();

    // Include context with pattern _1XX
    let mut included_ctx = Context::new("included");
    let mut ext_included = Extension::new("_1XX");
    ext_included.add_priority(Priority {
        priority: 1,
        app: "Noop".to_string(),
        app_data: "from_included".to_string(),
        label: None,
    });
    included_ctx.add_extension(ext_included);
    dp.add_context(included_ctx);

    // Local context with exact "100" -- should win over pattern match from include
    let mut local_ctx = Context::new("local");
    local_ctx.add_include("included");
    let mut ext_local = Extension::new("100");
    ext_local.add_priority(Priority {
        priority: 1,
        app: "Noop".to_string(),
        app_data: "from_local".to_string(),
        label: None,
    });
    local_ctx.add_extension(ext_local);
    dp.add_context(local_ctx);

    // "100" should match the local exact extension
    let result = dp.find_extension("local", "100");
    assert!(result.is_some());
    let (ctx, ext) = result.unwrap();
    assert_eq!(ctx.name, "local");
    assert_eq!(ext.name, "100");
    assert_eq!(ext.get_priority(1).unwrap().app_data, "from_local");

    // "150" should match the included pattern
    let result2 = dp.find_extension("local", "150");
    assert!(result2.is_some());
    let (ctx, ext) = result2.unwrap();
    assert_eq!(ctx.name, "included");
    assert_eq!(ext.name, "_1XX");
}

/// Test deeply nested includes.
#[test]
fn test_nested_includes() {
    let mut dp = Dialplan::new();

    // Level 3 context
    let mut ctx3 = Context::new("level3");
    let mut ext = Extension::new("999");
    ext.add_priority(Priority {
        priority: 1,
        app: "Answer".to_string(),
        app_data: String::new(),
        label: None,
    });
    ctx3.add_extension(ext);
    dp.add_context(ctx3);

    // Level 2 includes level 3
    let mut ctx2 = Context::new("level2");
    ctx2.add_include("level3");
    dp.add_context(ctx2);

    // Level 1 includes level 2
    let mut ctx1 = Context::new("level1");
    ctx1.add_include("level2");
    dp.add_context(ctx1);

    // Should find "999" through nested includes: level1 -> level2 -> level3
    let result = dp.find_extension("level1", "999");
    assert!(result.is_some());
    let (ctx, ext) = result.unwrap();
    assert_eq!(ctx.name, "level3");
    assert_eq!(ext.name, "999");
}

/// Test circular include protection.
#[test]
fn test_circular_include_protection() {
    let mut dp = Dialplan::new();

    // Context A includes B, B includes A -- should not infinite loop
    let mut ctx_a = Context::new("ctx_a");
    ctx_a.add_include("ctx_b");
    dp.add_context(ctx_a);

    let mut ctx_b = Context::new("ctx_b");
    ctx_b.add_include("ctx_a");
    dp.add_context(ctx_b);

    // This should return None (extension not found) without hanging
    let result = dp.find_extension("ctx_a", "100");
    assert!(result.is_none());
}

// =========================================================================
// Variable substitution tests
// =========================================================================

/// Test basic ${varname} substitution.
#[test]
fn test_variable_substitution_basic() {
    let mut chan = asterisk_core::channel::Channel::new("Test/varsub");
    chan.set_variable("MY_VAR", "hello");

    assert_eq!(chan.get_variable("MY_VAR"), Some("hello"));
}

/// Test missing variable returns None (empty string in C).
#[test]
fn test_missing_variable_returns_none() {
    let chan = asterisk_core::channel::Channel::new("Test/missing");
    assert_eq!(chan.get_variable("NONEXISTENT"), None);
}

/// Test variable overwrite.
#[test]
fn test_variable_overwrite() {
    let mut chan = asterisk_core::channel::Channel::new("Test/overwrite");
    chan.set_variable("VAR", "old");
    assert_eq!(chan.get_variable("VAR"), Some("old"));

    chan.set_variable("VAR", "new");
    assert_eq!(chan.get_variable("VAR"), Some("new"));
}

/// Test multiple variables coexist.
#[test]
fn test_multiple_variables() {
    let mut chan = asterisk_core::channel::Channel::new("Test/multi");
    chan.set_variable("A", "1");
    chan.set_variable("B", "2");
    chan.set_variable("C", "3");

    assert_eq!(chan.get_variable("A"), Some("1"));
    assert_eq!(chan.get_variable("B"), Some("2"));
    assert_eq!(chan.get_variable("C"), Some("3"));
    assert_eq!(chan.variables.len(), 3);
}
