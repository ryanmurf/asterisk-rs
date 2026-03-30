//! PBX / dialplan execution engine.

pub mod app_registry;
pub mod exec;
pub mod expression;
pub mod func_registry;
pub mod hints;
pub mod pbx_config;
pub mod pbx_realtime;
pub mod pbx_spool;
pub mod substitute;

use crate::channel::Channel;
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, LazyLock};

// ---------------------------------------------------------------------------
// Global dialplan singleton
// ---------------------------------------------------------------------------

/// Global dialplan -- set once at startup, read from Originate and other
/// subsystems that need to spawn `pbx_run`.
static GLOBAL_DIALPLAN: LazyLock<parking_lot::RwLock<Option<Arc<Dialplan>>>> =
    LazyLock::new(|| parking_lot::RwLock::new(None));

/// Store the loaded dialplan in the global singleton.
///
/// Called once during startup after `extensions.conf` has been parsed.
pub fn set_global_dialplan(dp: Arc<Dialplan>) {
    *GLOBAL_DIALPLAN.write() = Some(dp);
}

/// Retrieve the global dialplan (if set).
pub fn get_global_dialplan() -> Option<Arc<Dialplan>> {
    GLOBAL_DIALPLAN.read().clone()
}

// ---------------------------------------------------------------------------
// Global channel variables (like GLOBAL() function in Asterisk)
// ---------------------------------------------------------------------------

/// Global channel variables accessible via GetVar/SetVar AMI actions.
static GLOBAL_VARIABLES: LazyLock<parking_lot::RwLock<HashMap<String, String>>> =
    LazyLock::new(|| parking_lot::RwLock::new(HashMap::new()));

/// Set a global variable.
pub fn set_global_variable(name: String, value: String) {
    GLOBAL_VARIABLES.write().insert(name, value);
}

/// Get a global variable.
pub fn get_global_variable(name: &str) -> Option<String> {
    GLOBAL_VARIABLES.read().get(name).cloned()
}

/// Result of dialplan application execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PbxResult {
    /// Application completed successfully
    Success,
    /// Application failed
    Failed,
    /// Dialing is incomplete (need more digits)
    Incomplete,
}

/// A single priority step in an extension.
#[derive(Debug, Clone)]
pub struct Priority {
    /// Priority number (1, 2, 3, ... or special values like -1 for hint)
    pub priority: i32,
    /// Application name to execute
    pub app: String,
    /// Application data/arguments
    pub app_data: String,
    /// Label (optional, for GoTo)
    pub label: Option<String>,
}

/// An extension (pattern or literal) within a dialplan context.
#[derive(Debug, Clone)]
pub struct Extension {
    /// Extension name or pattern (e.g., "100", "_1XX", "_NXXNXXXXXX")
    pub name: String,
    /// Optional Caller ID match pattern.
    ///
    /// When set, this extension only matches if the caller ID also matches
    /// this pattern. CID patterns support the same `_X`/`_Z`/`_N`/`[range]`
    /// syntax as extension patterns.
    pub cidmatch: Option<String>,
    /// Priorities keyed by priority number
    pub priorities: HashMap<i32, Priority>,
}

impl Extension {
    /// Create a new extension.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            cidmatch: None,
            priorities: HashMap::new(),
        }
    }

    /// Create a new extension with a CID match pattern.
    pub fn new_with_cid(name: &str, cidmatch: &str) -> Self {
        Self {
            name: name.to_string(),
            cidmatch: Some(cidmatch.to_string()),
            priorities: HashMap::new(),
        }
    }

    /// Add a priority to this extension.
    pub fn add_priority(&mut self, prio: Priority) {
        self.priorities.insert(prio.priority, prio);
    }

    /// Get a priority by number.
    pub fn get_priority(&self, priority: i32) -> Option<&Priority> {
        self.priorities.get(&priority)
    }

    /// Get the next priority after the given one.
    pub fn next_priority(&self, current: i32) -> Option<&Priority> {
        self.priorities.get(&(current + 1))
    }

    /// Check if this extension matches the given string.
    /// Supports basic Asterisk pattern matching: _X, _Z, _N, _., _!
    pub fn matches(&self, exten: &str) -> bool {
        self.matches_with_cid(exten, None)
    }

    /// Check if this extension matches the given exten and optional caller ID.
    ///
    /// If the extension has a `cidmatch` pattern set, the caller ID must also
    /// match that pattern. If there is no `cidmatch`, any caller ID is accepted.
    pub fn matches_with_cid(&self, exten: &str, callerid: Option<&str>) -> bool {
        // First check extension name match
        let exten_matches = if self.name == exten {
            true
        } else if let Some(pattern) = self.name.strip_prefix('_') {
            pattern_matches(pattern, exten)
        } else {
            false
        };

        if !exten_matches {
            return false;
        }

        // Check CID match if required
        match (&self.cidmatch, callerid) {
            (None, _) => true, // No CID restriction
            (Some(_), None) => false, // CID required but not provided
            (Some(cid_pattern), Some(cid)) => {
                // Exact match
                if cid_pattern == cid {
                    return true;
                }
                // Pattern match
                if let Some(pattern) = cid_pattern.strip_prefix('_') {
                    pattern_matches(pattern, cid)
                } else {
                    false
                }
            }
        }
    }
}

/// A dialplan context containing extensions and includes.
#[derive(Debug, Clone)]
pub struct Context {
    /// Context name
    pub name: String,
    /// Extensions in this context
    pub extensions: HashMap<String, Extension>,
    /// Included contexts (searched in order after local extensions)
    pub includes: Vec<String>,
}

impl Context {
    /// Create a new context.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            extensions: HashMap::new(),
            includes: Vec::new(),
        }
    }

    /// Add an extension to this context.
    pub fn add_extension(&mut self, ext: Extension) {
        self.extensions.insert(ext.name.clone(), ext);
    }

    /// Add an include to this context.
    pub fn add_include(&mut self, context_name: impl Into<String>) {
        self.includes.push(context_name.into());
    }

    /// Find an extension matching the given string (exact match first, then patterns).
    pub fn find_extension(&self, exten: &str) -> Option<&Extension> {
        self.find_extension_with_cid(exten, None)
    }

    /// Find an extension matching the given exten and optional caller ID.
    ///
    /// Search order:
    /// 1. Exact name match with CID match (if CID provided)
    /// 2. Exact name match without CID restriction
    /// 3. Pattern match with CID match
    /// 4. Pattern match without CID restriction
    pub fn find_extension_with_cid(&self, exten: &str, callerid: Option<&str>) -> Option<&Extension> {
        // Exact match first (with CID)
        if let Some(ext) = self.extensions.get(exten) {
            if ext.matches_with_cid(exten, callerid) {
                return Some(ext);
            }
        }

        // Check all extensions for CID-specific matches first, then non-CID
        let mut best_match: Option<&Extension> = None;

        for ext in self.extensions.values() {
            if ext.matches_with_cid(exten, callerid) {
                // Prefer CID-specific matches over generic ones
                if ext.cidmatch.is_some() {
                    return Some(ext);
                }
                if best_match.is_none() {
                    best_match = Some(ext);
                }
            }
        }

        best_match
    }
}

/// The dialplan -- collection of contexts with extension-matching.
#[derive(Debug, Default)]
pub struct Dialplan {
    /// All contexts keyed by name
    pub contexts: HashMap<String, Context>,
}

impl Dialplan {
    /// Create a new empty dialplan.
    pub fn new() -> Self {
        Self {
            contexts: HashMap::new(),
        }
    }

    /// Add a context to the dialplan.
    pub fn add_context(&mut self, ctx: Context) {
        self.contexts.insert(ctx.name.clone(), ctx);
    }

    /// Get a context by name.
    pub fn get_context(&self, name: &str) -> Option<&Context> {
        self.contexts.get(name)
    }

    /// Get a mutable context by name.
    pub fn get_context_mut(&mut self, name: &str) -> Option<&mut Context> {
        self.contexts.get_mut(name)
    }

    /// Add an extension to a context (creating the context if needed).
    pub fn add_extension(&mut self, context_name: &str, ext: Extension) {
        self.contexts
            .entry(context_name.to_string())
            .or_insert_with(|| Context::new(context_name))
            .add_extension(ext);
    }

    /// Find an extension in a context (searches includes recursively).
    pub fn find_extension(&self, context_name: &str, exten: &str) -> Option<(&Context, &Extension)> {
        self.find_extension_recursive(context_name, exten, &mut Vec::new())
    }

    fn find_extension_recursive<'a>(
        &'a self,
        context_name: &str,
        exten: &str,
        visited: &mut Vec<String>,
    ) -> Option<(&'a Context, &'a Extension)> {
        // Prevent infinite loops
        if visited.contains(&context_name.to_string()) {
            return None;
        }
        visited.push(context_name.to_string());

        let ctx = self.contexts.get(context_name)?;

        // Search local extensions
        if let Some(ext) = ctx.find_extension(exten) {
            return Some((ctx, ext));
        }

        // Search includes
        for include in &ctx.includes {
            if let Some(result) = self.find_extension_recursive(include, exten, visited) {
                return Some(result);
            }
        }

        None
    }
}

/// Dialplan application trait.
///
/// Dialplan applications are the "verbs" of Asterisk -- Answer(), Dial(),
/// Playback(), Hangup(), etc.
#[async_trait::async_trait]
pub trait DialplanApp: Send + Sync + fmt::Debug {
    /// Application name (e.g., "Answer", "Dial", "Playback").
    fn name(&self) -> &str;

    /// Short description for help text.
    fn synopsis(&self) -> &str {
        ""
    }

    /// Execute the application on a channel with the given arguments.
    async fn execute(&self, channel: &mut Channel, args: &str) -> PbxResult;
}

/// Dialplan function trait.
///
/// Functions are used in expressions: ${FUNC(args)} for read, Set(FUNC(args)=value) for write.
#[async_trait::async_trait]
pub trait DialplanFunction: Send + Sync + fmt::Debug {
    /// Function name (e.g., "CALLERID", "CHANNEL", "LEN").
    fn name(&self) -> &str;

    /// Short description.
    fn synopsis(&self) -> &str {
        ""
    }

    /// Read the function value.
    async fn read(&self, channel: &Channel, args: &str) -> Result<String, String>;

    /// Write a value to the function.
    async fn write(&self, _channel: &mut Channel, _args: &str, _value: &str) -> Result<(), String> {
        Err(format!("Function {} does not support write", self.name()))
    }
}

/// Basic Asterisk dialplan pattern matching.
///
/// Pattern characters:
/// - X: any digit 0-9
/// - Z: any digit 1-9
/// - N: any digit 2-9
/// - [range]: character class (e.g., [1-5])
/// - .: match one or more remaining characters
/// - !: match zero or more remaining characters
fn pattern_matches(pattern: &str, input: &str) -> bool {
    let mut pat_chars = pattern.chars().peekable();
    let mut inp_chars = input.chars().peekable();

    loop {
        match (pat_chars.peek(), inp_chars.peek()) {
            (None, None) => return true,
            (None, Some(_)) => return false,
            (Some('.'), _) => return inp_chars.peek().is_some(), // . matches 1+ chars
            (Some('!'), _) => return true, // ! matches 0+ chars
            (Some(_), None) => return false,
            (Some(&pc), Some(&ic)) => {
                let matches = match pc {
                    'X' => ic.is_ascii_digit(),
                    'Z' => ic.is_ascii_digit() && ic != '0',
                    'N' => ic.is_ascii_digit() && ic != '0' && ic != '1',
                    '[' => {
                        // Parse character class [range]
                        pat_chars.next(); // consume '['
                        let mut class_matches = false;
                        let mut prev_char: Option<char> = None;
                        let mut in_range = false;

                        loop {
                            match pat_chars.peek() {
                                Some(']') | None => break,
                                Some('-') if prev_char.is_some() => {
                                    in_range = true;
                                    pat_chars.next();
                                    continue;
                                }
                                Some(&c) => {
                                    if in_range {
                                        if let Some(start) = prev_char {
                                            if ic >= start && ic <= c {
                                                class_matches = true;
                                            }
                                        }
                                        in_range = false;
                                    } else if c == ic {
                                        class_matches = true;
                                    }
                                    prev_char = Some(c);
                                    pat_chars.next();
                                }
                            }
                        }
                        // Consume the closing ']'
                        if pat_chars.peek() == Some(&']') {
                            pat_chars.next();
                        }
                        inp_chars.next();
                        if class_matches {
                            continue;
                        } else {
                            return false;
                        }
                    }
                    c => c == ic,
                };

                if !matches {
                    return false;
                }

                pat_chars.next();
                inp_chars.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_match() {
        let ext = Extension::new("100");
        assert!(ext.matches("100"));
        assert!(!ext.matches("101"));
    }

    #[test]
    fn test_pattern_x() {
        let ext = Extension::new("_1XX");
        assert!(ext.matches("100"));
        assert!(ext.matches("199"));
        assert!(!ext.matches("200"));
        assert!(!ext.matches("10")); // too short
    }

    #[test]
    fn test_pattern_n() {
        let ext = Extension::new("_NXX");
        assert!(ext.matches("200"));
        assert!(ext.matches("999"));
        assert!(!ext.matches("100")); // 1 not in N range
    }

    #[test]
    fn test_pattern_dot() {
        let ext = Extension::new("_1.");
        assert!(ext.matches("10"));
        assert!(ext.matches("12345"));
        assert!(!ext.matches("2")); // doesn't start with 1
    }

    #[test]
    fn test_pattern_bang() {
        let ext = Extension::new("_1!");
        assert!(ext.matches("1"));
        assert!(ext.matches("12345"));
        assert!(!ext.matches("2"));
    }

    #[test]
    fn test_dialplan_find() {
        let mut dp = Dialplan::new();
        let mut ctx = Context::new("default");
        let mut ext = Extension::new("100");
        ext.add_priority(Priority {
            priority: 1,
            app: "Answer".to_string(),
            app_data: String::new(),
            label: None,
        });
        ctx.add_extension(ext);
        dp.add_context(ctx);

        let result = dp.find_extension("default", "100");
        assert!(result.is_some());
        let (ctx, ext) = result.unwrap();
        assert_eq!(ctx.name, "default");
        assert_eq!(ext.name, "100");
    }

    #[test]
    fn test_cid_exact_match() {
        let ext = Extension::new_with_cid("100", "5551234");
        assert!(ext.matches_with_cid("100", Some("5551234")));
        assert!(!ext.matches_with_cid("100", Some("5559999")));
        assert!(!ext.matches_with_cid("100", None));
    }

    #[test]
    fn test_cid_pattern_match() {
        let ext = Extension::new_with_cid("100", "_555XXXX");
        assert!(ext.matches_with_cid("100", Some("5551234")));
        assert!(ext.matches_with_cid("100", Some("5559999")));
        assert!(!ext.matches_with_cid("100", Some("4441234")));
    }

    #[test]
    fn test_no_cid_restriction() {
        let ext = Extension::new("100");
        assert!(ext.matches_with_cid("100", Some("anything")));
        assert!(ext.matches_with_cid("100", None));
    }

    #[test]
    fn test_context_cid_priority() {
        let mut ctx = Context::new("default");

        // Extension with CID match
        let mut ext_cid = Extension::new_with_cid("100", "5551234");
        ext_cid.add_priority(Priority {
            priority: 1,
            app: "SpecialAnswer".to_string(),
            app_data: String::new(),
            label: None,
        });
        ctx.extensions.insert("100/5551234".to_string(), ext_cid);

        // Extension without CID match
        let mut ext_no_cid = Extension::new("100");
        ext_no_cid.add_priority(Priority {
            priority: 1,
            app: "Answer".to_string(),
            app_data: String::new(),
            label: None,
        });
        ctx.add_extension(ext_no_cid);

        // Without CID -> generic match
        let found = ctx.find_extension("100");
        assert!(found.is_some());
    }

    #[test]
    fn test_pattern_range_match() {
        let ext = Extension::new("_[2-5]XX");
        assert!(ext.matches("200"));
        assert!(ext.matches("500"));
        assert!(!ext.matches("100"));
        assert!(!ext.matches("600"));
    }

    #[test]
    fn test_dialplan_includes() {
        let mut dp = Dialplan::new();

        let mut default_ctx = Context::new("default");
        default_ctx.add_include("internal");
        dp.add_context(default_ctx);

        let mut internal_ctx = Context::new("internal");
        let mut ext = Extension::new("200");
        ext.add_priority(Priority {
            priority: 1,
            app: "Answer".to_string(),
            app_data: String::new(),
            label: None,
        });
        internal_ctx.add_extension(ext);
        dp.add_context(internal_ctx);

        // Should find 200 via include
        let result = dp.find_extension("default", "200");
        assert!(result.is_some());
    }
}
