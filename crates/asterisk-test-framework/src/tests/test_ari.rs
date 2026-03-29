//! Port of asterisk/tests/test_ari.c and test_ari_model.c
//!
//! Tests ARI (Asterisk REST Interface):
//! - REST resource routing
//! - Handler invocation
//! - JSON model validation (byte, boolean, int, long, double, string, date)
//! - Request parameter handling
//! - Path variable extraction

use serde_json::{json, Value};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// ARI model validators (port of ari_model_validators.h)
// ---------------------------------------------------------------------------

/// Validate that a JSON value is a valid byte (-128..=255).
fn validate_byte(val: &Value) -> bool {
    match val.as_i64() {
        Some(n) => (-128..=255).contains(&n),
        None => false,
    }
}

/// Validate that a JSON value is a valid boolean.
fn validate_boolean(val: &Value) -> bool {
    val.is_boolean()
}

/// Validate that a JSON value is a valid 32-bit integer.
fn validate_int(val: &Value) -> bool {
    match val.as_i64() {
        Some(n) => (i32::MIN as i64..=i32::MAX as i64).contains(&n),
        None => false,
    }
}

/// Validate that a JSON value is a valid 64-bit long.
fn validate_long(val: &Value) -> bool {
    val.as_i64().is_some()
}

/// Validate that a JSON value is a valid double/float.
fn validate_double(val: &Value) -> bool {
    val.is_f64() || val.is_i64() || val.is_u64()
}

/// Validate that a JSON value is a string.
fn validate_string(val: &Value) -> bool {
    val.is_string()
}

/// Validate that a JSON value is a valid ISO 8601 date.
fn validate_date(val: &Value) -> bool {
    match val.as_str() {
        Some(s) => {
            // Basic check: must look like a date.
            s.contains('-') && s.len() >= 10
        }
        None => false,
    }
}

// ---------------------------------------------------------------------------
// REST routing (simplified)
// ---------------------------------------------------------------------------

/// HTTP method enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
}

/// A handler response.
#[derive(Debug, Clone)]
struct AriResponse {
    code: u16,
    message: Value,
}

/// A route handler entry.
struct RouteHandler {
    path_segment: String,
    is_wildcard: bool,
    callbacks: HashMap<HttpMethod, fn(&HashMap<String, String>) -> AriResponse>,
    children: Vec<RouteHandler>,
}

impl RouteHandler {
    fn new(segment: &str) -> Self {
        Self {
            path_segment: segment.to_string(),
            is_wildcard: false,
            callbacks: HashMap::new(),
            children: Vec::new(),
        }
    }

    fn wildcard(segment: &str) -> Self {
        Self {
            path_segment: segment.to_string(),
            is_wildcard: true,
            callbacks: HashMap::new(),
            children: Vec::new(),
        }
    }

    fn add_callback(&mut self, method: HttpMethod, handler: fn(&HashMap<String, String>) -> AriResponse) {
        self.callbacks.insert(method, handler);
    }

    fn add_child(&mut self, child: RouteHandler) {
        self.children.push(child);
    }
}

/// Route a request path to the appropriate handler.
fn route_request(
    root: &RouteHandler,
    path: &[&str],
    method: HttpMethod,
    path_vars: &mut HashMap<String, String>,
) -> Option<AriResponse> {
    if path.is_empty() {
        return root.callbacks.get(&method).map(|h| h(path_vars));
    }

    let segment = path[0];
    let rest = &path[1..];

    // Check for exact match children first.
    for child in &root.children {
        if !child.is_wildcard && child.path_segment == segment {
            return route_request(child, rest, method, path_vars);
        }
    }

    // Check for wildcard children.
    for child in &root.children {
        if child.is_wildcard {
            path_vars.insert(child.path_segment.clone(), segment.to_string());
            return route_request(child, rest, method, path_vars);
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Model validation tests (from test_ari_model.c)
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(validate_byte) from test_ari_model.c.
///
/// Test byte validation with valid and invalid values.
#[test]
fn test_validate_byte() {
    assert!(validate_byte(&json!(-128)));
    assert!(validate_byte(&json!(0)));
    assert!(validate_byte(&json!(255)));

    assert!(!validate_byte(&json!(-129)));
    assert!(!validate_byte(&json!(256)));

    // String is not a byte.
    assert!(!validate_byte(&json!("not a byte")));
    assert!(!validate_byte(&json!("0")));

    // Null is not a byte.
    assert!(!validate_byte(&Value::Null));
}

/// Port of AST_TEST_DEFINE(validate_boolean) from test_ari_model.c.
///
/// Test boolean validation.
#[test]
fn test_validate_boolean() {
    assert!(validate_boolean(&json!(true)));
    assert!(validate_boolean(&json!(false)));

    // String is not a boolean.
    assert!(!validate_boolean(&json!("not a bool")));
    assert!(!validate_boolean(&json!("true")));

    // Null is not a boolean.
    assert!(!validate_boolean(&Value::Null));
}

/// Port of AST_TEST_DEFINE(validate_int) from test_ari_model.c.
///
/// Test 32-bit integer validation.
#[test]
fn test_validate_int() {
    assert!(validate_int(&json!(-2_147_483_648_i64)));
    assert!(validate_int(&json!(0)));
    assert!(validate_int(&json!(2_147_483_647_i64)));

    assert!(!validate_int(&json!(-2_147_483_649_i64)));
    assert!(!validate_int(&json!(2_147_483_648_i64)));

    // String is not an int.
    assert!(!validate_int(&json!("not an int")));
    assert!(!validate_int(&json!("0")));
    assert!(!validate_int(&Value::Null));
}

/// Test long (64-bit integer) validation.
#[test]
fn test_validate_long() {
    assert!(validate_long(&json!(0)));
    assert!(validate_long(&json!(i64::MIN)));
    assert!(validate_long(&json!(i64::MAX)));

    assert!(!validate_long(&json!("not a long")));
    assert!(!validate_long(&Value::Null));
}

/// Test double/float validation.
#[test]
fn test_validate_double() {
    assert!(validate_double(&json!(3.14)));
    assert!(validate_double(&json!(0.0)));
    assert!(validate_double(&json!(42))); // integers are also valid doubles

    assert!(!validate_double(&json!("not a double")));
    assert!(!validate_double(&Value::Null));
}

/// Test string validation.
#[test]
fn test_validate_string() {
    assert!(validate_string(&json!("hello")));
    assert!(validate_string(&json!("")));

    assert!(!validate_string(&json!(42)));
    assert!(!validate_string(&json!(true)));
    assert!(!validate_string(&Value::Null));
}

/// Test date validation.
#[test]
fn test_validate_date() {
    assert!(validate_date(&json!("2025-01-15T10:30:00Z")));
    assert!(validate_date(&json!("2025-01-15")));

    assert!(!validate_date(&json!("not a date")));
    assert!(!validate_date(&json!(42)));
    assert!(!validate_date(&Value::Null));
}

// ---------------------------------------------------------------------------
// REST routing tests (from test_ari.c)
// ---------------------------------------------------------------------------

fn foo_get_handler(_vars: &HashMap<String, String>) -> AriResponse {
    AriResponse {
        code: 200,
        message: json!({"name": "foo_get"}),
    }
}

fn bar_get_handler(_vars: &HashMap<String, String>) -> AriResponse {
    AriResponse {
        code: 200,
        message: json!({"name": "bar_get"}),
    }
}

fn bar_post_handler(_vars: &HashMap<String, String>) -> AriResponse {
    AriResponse {
        code: 200,
        message: json!({"name": "bar_post"}),
    }
}

fn bam_get_handler(vars: &HashMap<String, String>) -> AriResponse {
    AriResponse {
        code: 200,
        message: json!({"name": "bam_get", "bam": vars.get("bam").cloned().unwrap_or_default()}),
    }
}

fn bang_get_handler(_vars: &HashMap<String, String>) -> AriResponse {
    AriResponse {
        code: 200,
        message: json!({"name": "bang_get"}),
    }
}

fn bang_delete_handler(_vars: &HashMap<String, String>) -> AriResponse {
    AriResponse {
        code: 204,
        message: Value::Null,
    }
}

fn build_test_routes() -> RouteHandler {
    let mut foo = RouteHandler::new("foo");
    foo.add_callback(HttpMethod::Get, foo_get_handler);

    let mut bar = RouteHandler::new("bar");
    bar.add_callback(HttpMethod::Get, bar_get_handler);
    bar.add_callback(HttpMethod::Post, bar_post_handler);

    let mut bang = RouteHandler::new("bang");
    bang.add_callback(HttpMethod::Get, bang_get_handler);
    bang.add_callback(HttpMethod::Delete, bang_delete_handler);

    let mut bam = RouteHandler::wildcard("bam");
    bam.add_callback(HttpMethod::Get, bam_get_handler);
    bam.add_child(bang);

    foo.add_child(bar);
    foo.add_child(bam);

    foo
}

/// Test routing to /foo (GET).
#[test]
fn test_route_foo_get() {
    let root = build_test_routes();
    let mut vars = HashMap::new();
    let resp = route_request(&root, &[], HttpMethod::Get, &mut vars);
    assert!(resp.is_some());
    let resp = resp.unwrap();
    assert_eq!(resp.code, 200);
    assert_eq!(resp.message["name"], "foo_get");
}

/// Test routing to /foo/bar (GET).
#[test]
fn test_route_bar_get() {
    let root = build_test_routes();
    let mut vars = HashMap::new();
    let resp = route_request(&root, &["bar"], HttpMethod::Get, &mut vars);
    assert!(resp.is_some());
    assert_eq!(resp.unwrap().message["name"], "bar_get");
}

/// Test routing to /foo/bar (POST).
#[test]
fn test_route_bar_post() {
    let root = build_test_routes();
    let mut vars = HashMap::new();
    let resp = route_request(&root, &["bar"], HttpMethod::Post, &mut vars);
    assert!(resp.is_some());
    assert_eq!(resp.unwrap().message["name"], "bar_post");
}

/// Test routing to /foo/{bam} (GET) with wildcard.
#[test]
fn test_route_wildcard_bam() {
    let root = build_test_routes();
    let mut vars = HashMap::new();
    let resp = route_request(&root, &["myvalue"], HttpMethod::Get, &mut vars);
    assert!(resp.is_some());
    let resp = resp.unwrap();
    assert_eq!(resp.message["name"], "bam_get");
    assert_eq!(resp.message["bam"], "myvalue");
    assert_eq!(vars.get("bam").unwrap(), "myvalue");
}

/// Test routing to /foo/{bam}/bang (GET).
#[test]
fn test_route_bam_bang_get() {
    let root = build_test_routes();
    let mut vars = HashMap::new();
    let resp = route_request(&root, &["myvalue", "bang"], HttpMethod::Get, &mut vars);
    assert!(resp.is_some());
    assert_eq!(resp.unwrap().message["name"], "bang_get");
}

/// Test routing to /foo/{bam}/bang (DELETE).
#[test]
fn test_route_bam_bang_delete() {
    let root = build_test_routes();
    let mut vars = HashMap::new();
    let resp = route_request(&root, &["myvalue", "bang"], HttpMethod::Delete, &mut vars);
    assert!(resp.is_some());
    assert_eq!(resp.unwrap().code, 204);
}

/// Test routing to non-existent path returns None.
#[test]
fn test_route_not_found() {
    let root = build_test_routes();
    let mut vars = HashMap::new();
    let resp = route_request(&root, &["nonexistent", "deep", "path"], HttpMethod::Get, &mut vars);
    assert!(resp.is_none());
}

/// Test routing with wrong method returns None.
#[test]
fn test_route_method_not_allowed() {
    let root = build_test_routes();
    let mut vars = HashMap::new();
    // /foo only supports GET, not DELETE.
    let resp = route_request(&root, &[], HttpMethod::Delete, &mut vars);
    assert!(resp.is_none());
}
