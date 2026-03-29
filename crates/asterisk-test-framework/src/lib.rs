//! Asterisk Test Framework
//!
//! A port of the Asterisk C test framework (test.h) to Rust.
//! Provides test registration, execution, validation macros,
//! result reporting (text and JUnit XML), and mock channel technology
//! for integration testing.

pub mod mock_channel;

#[cfg(test)]
mod tests;

use parking_lot::Mutex;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// Information describing a test case, corresponding to `ast_test_info` in C.
#[derive(Debug, Clone)]
pub struct TestInfo {
    /// Name of the test, unique within its category.
    pub name: String,
    /// Category path (e.g. "/main/channel/").
    pub category: String,
    /// Short summary of what the test verifies.
    pub summary: String,
    /// More detailed description.
    pub description: String,
}

/// Result of running a single test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestResult {
    Pass,
    Fail,
    NotRun,
}

impl fmt::Display for TestResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TestResult::Pass => write!(f, "PASS"),
            TestResult::Fail => write!(f, "FAIL"),
            TestResult::NotRun => write!(f, "NOT_RUN"),
        }
    }
}

/// A single test case with its metadata, result, and log.
#[derive(Debug, Clone)]
pub struct TestCase {
    pub info: TestInfo,
    pub result: TestResult,
    pub duration: Duration,
    pub log: Vec<String>,
}

impl TestCase {
    /// Create a new not-yet-run test case.
    pub fn new(info: TestInfo) -> Self {
        TestCase {
            info,
            result: TestResult::NotRun,
            duration: Duration::ZERO,
            log: Vec::new(),
        }
    }

    /// Full identifier: "category/name"
    pub fn full_name(&self) -> String {
        format!("{}{}", self.info.category, self.info.name)
    }
}

/// Log a progress / status message into a test case.
pub fn test_status_update(test: &mut TestCase, msg: &str) {
    test.log.push(msg.to_string());
}

/// Validate a condition within a test, recording the source location on failure.
/// Returns true if the condition is true, false otherwise.
pub fn test_validate(test: &mut TestCase, condition: bool, file: &str, line: u32) -> bool {
    if !condition {
        let msg = format!("{}:{}: Condition failed", file, line);
        test.log.push(msg);
        test.result = TestResult::Fail;
    }
    condition
}

/// Validate a condition inside a test, auto-filling file and line.
/// On failure the test result is set to Fail and false is returned.
#[macro_export]
macro_rules! ast_test_validate {
    ($test:expr, $condition:expr) => {
        if !$crate::test_validate($test, $condition, file!(), line!()) {
            return;
        }
    };
    ($test:expr, $condition:expr, $msg:expr) => {
        if !$crate::test_validate($test, $condition, file!(), line!()) {
            $crate::test_status_update($test, $msg);
            return;
        }
    };
}

// ---------------------------------------------------------------------------
// Test Registry & Runner
// ---------------------------------------------------------------------------

/// Type alias for a test function that operates on a TestCase.
pub type TestFn = Box<dyn Fn(&mut TestCase) + Send + Sync>;

/// Global test registry. Tests are registered by category+name and can be
/// executed individually, by category, or all at once.
pub struct TestRegistry {
    tests: Mutex<HashMap<String, (TestInfo, TestFn)>>,
}

impl TestRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        TestRegistry {
            tests: Mutex::new(HashMap::new()),
        }
    }

    /// Register a test with its info and callback.
    pub fn register(&self, info: TestInfo, f: TestFn) {
        let key = format!("{}{}", info.category, info.name);
        self.tests.lock().insert(key, (info, f));
    }

    /// Execute all registered tests.
    pub fn execute_all(&self) -> Vec<TestCase> {
        let tests = self.tests.lock();
        let mut results = Vec::new();
        for (_, (info, f)) in tests.iter() {
            let tc = Self::run_one(info, f);
            results.push(tc);
        }
        results
    }

    /// Execute all tests whose category starts with the given prefix.
    pub fn execute_category(&self, category_prefix: &str) -> Vec<TestCase> {
        let tests = self.tests.lock();
        let mut results = Vec::new();
        for (_, (info, f)) in tests.iter() {
            if info.category.starts_with(category_prefix) {
                results.push(Self::run_one(info, f));
            }
        }
        results
    }

    /// Execute a single test by its full name (category + name).
    pub fn execute_by_name(&self, full_name: &str) -> Option<TestCase> {
        let tests = self.tests.lock();
        tests.get(full_name).map(|(info, f)| Self::run_one(info, f))
    }

    fn run_one(info: &TestInfo, f: &TestFn) -> TestCase {
        let mut tc = TestCase::new(info.clone());
        tc.result = TestResult::Pass; // assume pass unless test sets Fail
        let start = Instant::now();
        f(&mut tc);
        tc.duration = start.elapsed();
        tc
    }
}

impl Default for TestRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Test Runner
// ---------------------------------------------------------------------------

/// A test runner that executes tests and collects results.
pub struct TestRunner {
    pub registry: Arc<TestRegistry>,
}

impl TestRunner {
    /// Create a new runner wrapping a registry.
    pub fn new(registry: Arc<TestRegistry>) -> Self {
        TestRunner { registry }
    }

    /// Run all tests, print a text summary, and return results.
    pub fn run_all(&self) -> Vec<TestCase> {
        let results = self.registry.execute_all();
        Self::print_summary(&results);
        results
    }

    /// Run tests matching a category prefix.
    pub fn run_category(&self, prefix: &str) -> Vec<TestCase> {
        let results = self.registry.execute_category(prefix);
        Self::print_summary(&results);
        results
    }

    fn print_summary(results: &[TestCase]) {
        let total = results.len();
        let passed = results.iter().filter(|t| t.result == TestResult::Pass).count();
        let failed = results.iter().filter(|t| t.result == TestResult::Fail).count();
        let not_run = results.iter().filter(|t| t.result == TestResult::NotRun).count();

        println!("=== Test Results ===");
        for tc in results {
            println!(
                "  [{}] {} ({:.3}s)",
                tc.result,
                tc.full_name(),
                tc.duration.as_secs_f64(),
            );
            for msg in &tc.log {
                println!("       {}", msg);
            }
        }
        println!(
            "=== Total: {} | Pass: {} | Fail: {} | NotRun: {} ===",
            total, passed, failed, not_run
        );
    }
}

// ---------------------------------------------------------------------------
// Reporting -- JUnit XML and text
// ---------------------------------------------------------------------------

/// Generate a JUnit-compatible XML report from test results.
pub fn generate_junit_xml(results: &[TestCase]) -> String {
    let total = results.len();
    let failures = results.iter().filter(|t| t.result == TestResult::Fail).count();
    let skipped = results.iter().filter(|t| t.result == TestResult::NotRun).count();
    let total_time: f64 = results.iter().map(|t| t.duration.as_secs_f64()).sum();

    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    xml.push_str(&format!(
        "<testsuite name=\"asterisk-tests\" tests=\"{}\" failures=\"{}\" skipped=\"{}\" time=\"{:.3}\">\n",
        total, failures, skipped, total_time
    ));

    for tc in results {
        xml.push_str(&format!(
            "  <testcase classname=\"{}\" name=\"{}\" time=\"{:.3}\"",
            escape_xml(&tc.info.category),
            escape_xml(&tc.info.name),
            tc.duration.as_secs_f64()
        ));

        match tc.result {
            TestResult::Fail => {
                xml.push_str(">\n");
                xml.push_str("    <failure message=\"Test failed\">");
                for msg in &tc.log {
                    xml.push_str(&escape_xml(msg));
                    xml.push('\n');
                }
                xml.push_str("</failure>\n");
                xml.push_str("  </testcase>\n");
            }
            TestResult::NotRun => {
                xml.push_str(">\n");
                xml.push_str("    <skipped/>\n");
                xml.push_str("  </testcase>\n");
            }
            TestResult::Pass => {
                xml.push_str("/>\n");
            }
        }
    }

    xml.push_str("</testsuite>\n");
    xml
}

/// Generate a text summary report from test results.
pub fn generate_text_report(results: &[TestCase]) -> String {
    let mut report = String::new();
    report.push_str("Asterisk Test Report\n");
    report.push_str("====================\n\n");

    let total = results.len();
    let passed = results.iter().filter(|t| t.result == TestResult::Pass).count();
    let failed = results.iter().filter(|t| t.result == TestResult::Fail).count();
    let not_run = results.iter().filter(|t| t.result == TestResult::NotRun).count();

    report.push_str(&format!(
        "Total: {}  Pass: {}  Fail: {}  NotRun: {}\n\n",
        total, passed, failed, not_run
    ));

    for tc in results {
        report.push_str(&format!(
            "[{}] {} - {} ({:.3}s)\n",
            tc.result,
            tc.full_name(),
            tc.info.summary,
            tc.duration.as_secs_f64()
        ));
        for msg in &tc.log {
            report.push_str(&format!("  LOG: {}\n", msg));
        }
    }

    report
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
