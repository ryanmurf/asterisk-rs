//! Port of asterisk/tests/test_config.c
//!
//! Tests configuration file parsing: sections, variables, templates,
//! category iteration, variable lookup, comment handling, and
//! object vs variable assignment.

use asterisk_config::AsteriskConfig;

/// Port of the build_cfg / test_config_validity pattern from test_config.c.
///
/// Creates a config with two categories (Capitals and Protagonists) and
/// verifies all variables are correctly stored and retrievable.
#[test]
fn test_config_build_and_validate() {
    let content = r#"
[Capitals]
Germany = Berlin
China = Beijing
Canada = Ottawa

[Protagonists]
1984 = Winston Smith
Green Eggs And Ham = Sam I Am
The Kalevala = Vainamoinen
"#;

    let config = AsteriskConfig::from_str(content, "test.conf").unwrap();

    // Verify categories
    let names = config.category_names();
    assert_eq!(names.len(), 2);
    assert_eq!(names[0], "Capitals");
    assert_eq!(names[1], "Protagonists");

    // Verify Capitals
    assert_eq!(config.get_variable("Capitals", "Germany"), Some("Berlin"));
    assert_eq!(config.get_variable("Capitals", "China"), Some("Beijing"));
    assert_eq!(config.get_variable("Capitals", "Canada"), Some("Ottawa"));

    // Verify Protagonists
    assert_eq!(config.get_variable("Protagonists", "1984"), Some("Winston Smith"));
    assert_eq!(
        config.get_variable("Protagonists", "Green Eggs And Ham"),
        Some("Sam I Am")
    );
    assert_eq!(
        config.get_variable("Protagonists", "The Kalevala"),
        Some("Vainamoinen")
    );
}

/// Port of copy_config test: copy a config and verify it matches.
///
/// In Rust, we clone the config and verify the clone matches the original.
#[test]
fn test_config_copy() {
    let content = r#"
[Capitals]
Germany = Berlin
China = Beijing

[Protagonists]
1984 = Winston Smith
"#;

    let config = AsteriskConfig::from_str(content, "test.conf").unwrap();
    let copy = config.clone();

    // Verify the copy has the same data
    assert_eq!(copy.categories.len(), config.categories.len());
    assert_eq!(
        copy.get_variable("Capitals", "Germany"),
        config.get_variable("Capitals", "Germany")
    );
    assert_eq!(
        copy.get_variable("Protagonists", "1984"),
        config.get_variable("Protagonists", "1984")
    );
}

/// Test template inheritance.
///
/// Port of template behavior from test_config.c where [child](parent)
/// inherits variables from the parent template [parent](!).
#[test]
fn test_template_inheritance() {
    let content = r#"
[base-phone](!)
type = friend
host = dynamic
context = default

[phone1](base-phone)
secret = pass123
callerid = "Phone 1" <100>

[phone2](base-phone)
secret = pass456
callerid = "Phone 2" <200>
"#;

    let config = AsteriskConfig::from_str(content, "test.conf").unwrap();

    // Template should be marked as template
    let base = config.get_category("base-phone").unwrap();
    assert!(base.is_template);
    assert_eq!(base.get_variable("type"), Some("friend"));

    // phone1 should inherit from base-phone
    let phone1 = config.get_category("phone1").unwrap();
    assert!(!phone1.is_template);
    assert_eq!(phone1.template_name.as_deref(), Some("base-phone"));
    assert_eq!(phone1.get_variable("type"), Some("friend")); // inherited
    assert_eq!(phone1.get_variable("host"), Some("dynamic")); // inherited
    assert_eq!(phone1.get_variable("context"), Some("default")); // inherited
    assert_eq!(phone1.get_variable("secret"), Some("pass123")); // own

    // phone2 should also inherit
    let phone2 = config.get_category("phone2").unwrap();
    assert_eq!(phone2.get_variable("type"), Some("friend")); // inherited
    assert_eq!(phone2.get_variable("secret"), Some("pass456")); // own
}

/// Test category iteration.
///
/// Port of ast_category_browse iteration test from test_config.c.
#[test]
fn test_category_iteration() {
    let content = r#"
[section1]
key1 = val1

[section2]
key2 = val2

[section3]
key3 = val3
"#;

    let config = AsteriskConfig::from_str(content, "test.conf").unwrap();
    let categories = config.get_categories();
    assert_eq!(categories.len(), 3);
    assert_eq!(categories[0].name, "section1");
    assert_eq!(categories[1].name, "section2");
    assert_eq!(categories[2].name, "section3");
}

/// Test variable lookup by name.
///
/// Port of ast_variable_find in test_config.c.
#[test]
fn test_variable_lookup() {
    let content = r#"
[mycat]
first = one
second = two
third = three
"#;

    let config = AsteriskConfig::from_str(content, "test.conf").unwrap();
    let cat = config.get_category("mycat").unwrap();

    assert_eq!(cat.get_variable("first"), Some("one"));
    assert_eq!(cat.get_variable("second"), Some("two"));
    assert_eq!(cat.get_variable("third"), Some("three"));
    assert_eq!(cat.get_variable("nonexistent"), None);
}

/// Test comment handling.
///
/// Port of comment behavior from the config parser. Lines starting with ;
/// or // should be ignored. Inline comments after ; should also be stripped.
#[test]
fn test_comment_handling() {
    let content = r#"
; This is a full-line comment
// This is also a comment
[general]
key = value ; inline comment
; another comment
name = test
"#;

    let config = AsteriskConfig::from_str(content, "test.conf").unwrap();
    assert_eq!(config.get_variable("general", "key"), Some("value"));
    assert_eq!(config.get_variable("general", "name"), Some("test"));
    // Only one category and two variables
    assert_eq!(config.categories.len(), 1);
    assert_eq!(config.get_category("general").unwrap().variables.len(), 2);
}

/// Test object vs variable assignment (= vs =>).
///
/// Port of the distinction between regular assignment (=) and object
/// assignment (=>) in Asterisk config files.
#[test]
fn test_object_vs_variable_assignment() {
    let content = r#"
[extensions]
exten => 100,1,Answer()
exten => 100,2,Hangup()

[general]
context = default
bindport = 5060
"#;

    let config = AsteriskConfig::from_str(content, "test.conf").unwrap();

    // Object assignments (=>)
    let ext = config.get_category("extensions").unwrap();
    assert_eq!(ext.variables.len(), 2);
    assert!(ext.variables[0].is_object);
    assert_eq!(ext.variables[0].name, "exten");
    assert_eq!(ext.variables[0].value, "100,1,Answer()");
    assert!(ext.variables[1].is_object);

    // Regular assignments (=)
    let gen = config.get_category("general").unwrap();
    assert!(!gen.variables[0].is_object);
    assert_eq!(gen.variables[0].name, "context");
    assert_eq!(gen.variables[0].value, "default");
}

/// Test empty config.
#[test]
fn test_empty_config() {
    let content = "";
    let config = AsteriskConfig::from_str(content, "empty.conf").unwrap();
    assert_eq!(config.categories.len(), 0);
    assert!(config.category_names().is_empty());
}

/// Test category with no variables.
#[test]
fn test_empty_category() {
    let content = r#"
[empty_section]
"#;
    let config = AsteriskConfig::from_str(content, "test.conf").unwrap();
    let cat = config.get_category("empty_section").unwrap();
    assert_eq!(cat.variables.len(), 0);
}

/// Test multiple variables with the same name (multi-value).
#[test]
fn test_multi_value_variables() {
    let content = r#"
[peers]
allow = ulaw
allow = alaw
allow = g722
"#;

    let config = AsteriskConfig::from_str(content, "test.conf").unwrap();
    let cat = config.get_category("peers").unwrap();
    let allows = cat.get_all_variables("allow");
    assert_eq!(allows.len(), 3);
    assert_eq!(allows[0], "ulaw");
    assert_eq!(allows[1], "alaw");
    assert_eq!(allows[2], "g722");
}

/// Test category case-insensitive lookup.
#[test]
fn test_case_insensitive_category_lookup() {
    let content = r#"
[General]
key = val
"#;

    let config = AsteriskConfig::from_str(content, "test.conf").unwrap();
    // Category lookup should be case-insensitive
    assert!(config.get_category("General").is_some());
    assert!(config.get_category("general").is_some());
    assert!(config.get_category("GENERAL").is_some());
}

/// Test variable case-insensitive lookup.
#[test]
fn test_case_insensitive_variable_lookup() {
    let content = r#"
[section]
MyKey = myvalue
"#;

    let config = AsteriskConfig::from_str(content, "test.conf").unwrap();
    let cat = config.get_category("section").unwrap();
    assert_eq!(cat.get_variable("MyKey"), Some("myvalue"));
    assert_eq!(cat.get_variable("mykey"), Some("myvalue"));
    assert_eq!(cat.get_variable("MYKEY"), Some("myvalue"));
}

/// Test variable_names returns unique names in order.
#[test]
fn test_variable_names() {
    let content = r#"
[section]
alpha = 1
beta = 2
alpha = 3
gamma = 4
"#;

    let config = AsteriskConfig::from_str(content, "test.conf").unwrap();
    let cat = config.get_category("section").unwrap();
    let names = cat.variable_names();
    assert_eq!(names, vec!["alpha", "beta", "gamma"]);
}
