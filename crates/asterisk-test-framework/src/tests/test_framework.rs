//! Tests for the test framework itself.

use crate::*;
use std::sync::Arc;

#[test]
fn test_info_creation() {
    let info = TestInfo {
        name: "my_test".to_string(),
        category: "/main/test/".to_string(),
        summary: "a summary".to_string(),
        description: "a description".to_string(),
    };
    assert_eq!(info.name, "my_test");
    assert_eq!(info.category, "/main/test/");
}

#[test]
fn test_case_creation() {
    let info = TestInfo {
        name: "test1".to_string(),
        category: "/main/channel/".to_string(),
        summary: "test".to_string(),
        description: "test".to_string(),
    };
    let tc = TestCase::new(info);
    assert_eq!(tc.result, TestResult::NotRun);
    assert_eq!(tc.full_name(), "/main/channel/test1");
    assert!(tc.log.is_empty());
}

#[test]
fn test_status_update_appends() {
    let info = TestInfo {
        name: "t".to_string(),
        category: "/".to_string(),
        summary: "s".to_string(),
        description: "d".to_string(),
    };
    let mut tc = TestCase::new(info);
    test_status_update(&mut tc, "first message");
    test_status_update(&mut tc, "second message");
    assert_eq!(tc.log.len(), 2);
    assert_eq!(tc.log[0], "first message");
    assert_eq!(tc.log[1], "second message");
}

#[test]
fn test_validate_pass() {
    let info = TestInfo {
        name: "t".to_string(),
        category: "/".to_string(),
        summary: "s".to_string(),
        description: "d".to_string(),
    };
    let mut tc = TestCase::new(info);
    tc.result = TestResult::Pass;
    let ok = test_validate(&mut tc, true, "file.rs", 42);
    assert!(ok);
    assert_eq!(tc.result, TestResult::Pass);
}

#[test]
fn test_validate_fail() {
    let info = TestInfo {
        name: "t".to_string(),
        category: "/".to_string(),
        summary: "s".to_string(),
        description: "d".to_string(),
    };
    let mut tc = TestCase::new(info);
    tc.result = TestResult::Pass;
    let ok = test_validate(&mut tc, false, "file.rs", 42);
    assert!(!ok);
    assert_eq!(tc.result, TestResult::Fail);
    assert!(tc.log[0].contains("file.rs:42"));
}

#[test]
fn test_registry_register_and_execute() {
    let registry = Arc::new(TestRegistry::new());
    let info = TestInfo {
        name: "pass_test".to_string(),
        category: "/test/".to_string(),
        summary: "always passes".to_string(),
        description: "a test that always passes".to_string(),
    };
    registry.register(info, Box::new(|tc| {
        // Test passes by default (result is set to Pass before callback)
        test_status_update(tc, "running pass_test");
    }));

    let results = registry.execute_all();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].result, TestResult::Pass);
    assert_eq!(results[0].log.len(), 1);
}

#[test]
fn test_registry_execute_by_name() {
    let registry = Arc::new(TestRegistry::new());
    let info = TestInfo {
        name: "named_test".to_string(),
        category: "/cat/".to_string(),
        summary: "s".to_string(),
        description: "d".to_string(),
    };
    registry.register(info, Box::new(|_tc| {}));

    assert!(registry.execute_by_name("/cat/named_test").is_some());
    assert!(registry.execute_by_name("/cat/nonexistent").is_none());
}

#[test]
fn test_registry_execute_category() {
    let registry = Arc::new(TestRegistry::new());

    registry.register(
        TestInfo {
            name: "a".to_string(),
            category: "/main/channel/".to_string(),
            summary: "s".to_string(),
            description: "d".to_string(),
        },
        Box::new(|_| {}),
    );
    registry.register(
        TestInfo {
            name: "b".to_string(),
            category: "/main/pbx/".to_string(),
            summary: "s".to_string(),
            description: "d".to_string(),
        },
        Box::new(|_| {}),
    );
    registry.register(
        TestInfo {
            name: "c".to_string(),
            category: "/main/channel/".to_string(),
            summary: "s".to_string(),
            description: "d".to_string(),
        },
        Box::new(|_| {}),
    );

    let results = registry.execute_category("/main/channel/");
    assert_eq!(results.len(), 2);
}

#[test]
fn test_junit_xml_generation() {
    let results = vec![
        TestCase {
            info: TestInfo {
                name: "pass_test".to_string(),
                category: "/main/test/".to_string(),
                summary: "passes".to_string(),
                description: "d".to_string(),
            },
            result: TestResult::Pass,
            duration: std::time::Duration::from_millis(42),
            log: vec![],
        },
        TestCase {
            info: TestInfo {
                name: "fail_test".to_string(),
                category: "/main/test/".to_string(),
                summary: "fails".to_string(),
                description: "d".to_string(),
            },
            result: TestResult::Fail,
            duration: std::time::Duration::from_millis(100),
            log: vec!["error message".to_string()],
        },
    ];

    let xml = generate_junit_xml(&results);
    assert!(xml.contains("testsuite"));
    assert!(xml.contains("tests=\"2\""));
    assert!(xml.contains("failures=\"1\""));
    assert!(xml.contains("pass_test"));
    assert!(xml.contains("fail_test"));
    assert!(xml.contains("error message"));
}

#[test]
fn test_text_report_generation() {
    let results = vec![TestCase {
        info: TestInfo {
            name: "my_test".to_string(),
            category: "/main/".to_string(),
            summary: "a summary".to_string(),
            description: "d".to_string(),
        },
        result: TestResult::Pass,
        duration: std::time::Duration::from_millis(10),
        log: vec![],
    }];

    let report = generate_text_report(&results);
    assert!(report.contains("Asterisk Test Report"));
    assert!(report.contains("Pass: 1"));
    assert!(report.contains("my_test"));
}

#[test]
fn test_runner_run_all() {
    let registry = Arc::new(TestRegistry::new());
    registry.register(
        TestInfo {
            name: "t1".to_string(),
            category: "/".to_string(),
            summary: "s".to_string(),
            description: "d".to_string(),
        },
        Box::new(|_| {}),
    );
    let runner = TestRunner::new(registry);
    let results = runner.run_all();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].result, TestResult::Pass);
}
