use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors that can occur during configuration parsing.
#[derive(Error, Debug)]
pub enum ConfigError {
    /// File could not be read
    #[error("could not read config file '{path}': {source}")]
    FileRead {
        path: String,
        source: std::io::Error,
    },

    /// Parse error at a specific line
    #[error("{file}:{line}: {message}")]
    ParseError {
        file: String,
        line: usize,
        message: String,
    },

    /// Included file not found
    #[error("included file not found: {0}")]
    IncludeNotFound(String),

    /// Template not found
    #[error("template not found: {0}")]
    TemplateNotFound(String),

    /// Category not found
    #[error("category not found: {0}")]
    CategoryNotFound(String),

    /// Variable not found
    #[error("variable not found: {0}")]
    VariableNotFound(String),
}

/// A single configuration variable (key=value pair).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Variable {
    /// Variable name
    pub name: String,
    /// Variable value
    pub value: String,
    /// File where this variable was defined
    pub file: String,
    /// Line number where this variable was defined
    pub lineno: usize,
    /// Whether this is an "object" assignment (key => value) vs regular (key = value)
    pub is_object: bool,
    /// Whether this was inherited from a template
    pub inherited: bool,
}

/// A configuration category (section), e.g. `[general]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Category {
    /// Category name (the text between square brackets)
    pub name: String,
    /// Whether this category is a template (has `!` suffix in definition)
    pub is_template: bool,
    /// Variables in this category, in order of appearance
    pub variables: Vec<Variable>,
    /// Name of the template this category inherits from, if any
    pub template_name: Option<String>,
    /// File where this category was defined
    pub file: String,
    /// Line number where this category was defined
    pub lineno: usize,
}

impl Category {
    /// Get the first variable with the given name.
    pub fn get_variable(&self, name: &str) -> Option<&str> {
        self.variables
            .iter()
            .find(|v| v.name.eq_ignore_ascii_case(name))
            .map(|v| v.value.as_str())
    }

    /// Get all variables with the given name (for multi-value keys).
    pub fn get_all_variables(&self, name: &str) -> Vec<&str> {
        self.variables
            .iter()
            .filter(|v| v.name.eq_ignore_ascii_case(name))
            .map(|v| v.value.as_str())
            .collect()
    }

    /// Get all variable names in this category (unique, preserving first-seen order).
    pub fn variable_names(&self) -> Vec<&str> {
        let mut seen = std::collections::HashSet::new();
        let mut names = Vec::new();
        for v in &self.variables {
            if seen.insert(v.name.to_lowercase()) {
                names.push(v.name.as_str());
            }
        }
        names
    }
}

/// A parsed Asterisk configuration file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsteriskConfig {
    /// The primary filename that was loaded
    pub filename: String,
    /// All categories in order of appearance
    pub categories: Vec<Category>,
    /// Index from category name to indices in the categories Vec
    #[serde(skip)]
    category_index: HashMap<String, Vec<usize>>,
}

impl AsteriskConfig {
    /// Load and parse a configuration file from the given path.
    ///
    /// This handles:
    /// - `[section]` headers (categories)
    /// - `[section](!)` template definitions
    /// - `[section](template_name)` template inheritance
    /// - `key = value` assignments
    /// - `key => value` object assignments
    /// - `;` comment lines
    /// - `#include "filename"` directives
    /// - Blank lines are ignored
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path).map_err(|e| ConfigError::FileRead {
            path: path.display().to_string(),
            source: e,
        })?;

        let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
        let filename = path.display().to_string();

        let mut config = AsteriskConfig {
            filename: filename.clone(),
            categories: Vec::new(),
            category_index: HashMap::new(),
        };

        Parser::parse_content(&content, &filename, base_dir, &mut config)?;
        config.rebuild_index();

        // Apply template inheritance
        config.apply_templates()?;

        Ok(config)
    }

    /// Parse configuration from a string (useful for testing).
    pub fn from_str(content: &str, filename: &str) -> Result<Self, ConfigError> {
        let mut config = AsteriskConfig {
            filename: filename.to_string(),
            categories: Vec::new(),
            category_index: HashMap::new(),
        };

        Parser::parse_content(content, filename, Path::new("."), &mut config)?;
        config.rebuild_index();
        config.apply_templates()?;

        Ok(config)
    }

    /// Get the first category with the given name.
    pub fn get_category(&self, name: &str) -> Option<&Category> {
        self.category_index
            .get(&name.to_lowercase())
            .and_then(|indices| indices.first())
            .map(|&idx| &self.categories[idx])
    }

    /// Get all categories with the given name (for configs that allow duplicate sections).
    pub fn get_categories_by_name(&self, name: &str) -> Vec<&Category> {
        self.category_index
            .get(&name.to_lowercase())
            .map(|indices| indices.iter().map(|&idx| &self.categories[idx]).collect())
            .unwrap_or_default()
    }

    /// Get all categories in order.
    pub fn get_categories(&self) -> &[Category] {
        &self.categories
    }

    /// Get all category names (unique, in order of first appearance).
    pub fn category_names(&self) -> Vec<&str> {
        let mut seen = std::collections::HashSet::new();
        let mut names = Vec::new();
        for cat in &self.categories {
            if seen.insert(cat.name.to_lowercase()) {
                names.push(cat.name.as_str());
            }
        }
        names
    }

    /// Shorthand to get a variable value from a specific category.
    pub fn get_variable(&self, category: &str, variable: &str) -> Option<&str> {
        self.get_category(category)
            .and_then(|cat| cat.get_variable(variable))
    }

    fn rebuild_index(&mut self) {
        self.category_index.clear();
        for (idx, cat) in self.categories.iter().enumerate() {
            self.category_index
                .entry(cat.name.to_lowercase())
                .or_default()
                .push(idx);
        }
    }

    fn apply_templates(&mut self) -> Result<(), ConfigError> {
        // Collect template data first to avoid borrow issues
        let template_vars: HashMap<String, Vec<Variable>> = self
            .categories
            .iter()
            .filter(|c| c.is_template)
            .map(|c| (c.name.to_lowercase(), c.variables.clone()))
            .collect();

        // Apply templates to categories that inherit from them
        for cat in &mut self.categories {
            if let Some(ref tmpl_name) = cat.template_name {
                let key = tmpl_name.to_lowercase();
                if let Some(tmpl_vars) = template_vars.get(&key) {
                    // Prepend template variables (inherited vars come first)
                    let mut inherited: Vec<Variable> = tmpl_vars
                        .iter()
                        .map(|v| Variable {
                            inherited: true,
                            ..v.clone()
                        })
                        .collect();
                    inherited.append(&mut cat.variables);
                    cat.variables = inherited;
                }
                // If template not found, we silently skip (matching Asterisk behavior
                // where missing templates log a warning but don't fail)
            }
        }

        Ok(())
    }
}

/// Maximum recursion depth for #include directives to prevent stack overflow.
const MAX_INCLUDE_DEPTH: usize = 16;

/// Internal parser state.
struct Parser;

impl Parser {
    fn parse_content(
        content: &str,
        filename: &str,
        base_dir: &Path,
        config: &mut AsteriskConfig,
    ) -> Result<(), ConfigError> {
        Self::parse_content_inner(content, filename, base_dir, config, 0)
    }

    fn parse_content_inner(
        content: &str,
        filename: &str,
        base_dir: &Path,
        config: &mut AsteriskConfig,
        depth: usize,
    ) -> Result<(), ConfigError> {
        if depth > MAX_INCLUDE_DEPTH {
            return Err(ConfigError::ParseError {
                file: filename.to_string(),
                line: 0,
                message: format!(
                    "#include depth exceeds maximum ({}) -- possible cycle",
                    MAX_INCLUDE_DEPTH
                ),
            });
        }

        let mut current_category: Option<Category> = None;

        for (line_idx, raw_line) in content.lines().enumerate() {
            let lineno = line_idx + 1;
            let line = raw_line.trim();

            // Skip empty lines
            if line.is_empty() {
                continue;
            }

            // Skip comment lines (starting with ; or //)
            if line.starts_with(';') || line.starts_with("//") {
                continue;
            }

            // Strip inline comments (but not inside quoted values)
            let line = strip_inline_comment(line);
            let line = line.trim();

            if line.is_empty() {
                continue;
            }

            // Handle #include directives
            if line.starts_with("#include") {
                // Save current category before processing include
                if let Some(cat) = current_category.take() {
                    config.categories.push(cat);
                }

                let include_path = line
                    .strip_prefix("#include")
                    .unwrap()
                    .trim()
                    .trim_matches('"')
                    .trim_matches('<')
                    .trim_matches('>');

                if include_path.is_empty() {
                    return Err(ConfigError::ParseError {
                        file: filename.to_string(),
                        line: lineno,
                        message: "empty #include path".to_string(),
                    });
                }

                // Security: reject path traversal attempts
                if include_path.contains("..") {
                    return Err(ConfigError::ParseError {
                        file: filename.to_string(),
                        line: lineno,
                        message: format!(
                            "#include path contains '..': {}",
                            include_path
                        ),
                    });
                }

                // Security: reject absolute paths to prevent reading
                // arbitrary system files (e.g., /etc/passwd)
                if Path::new(include_path).is_absolute() {
                    return Err(ConfigError::ParseError {
                        file: filename.to_string(),
                        line: lineno,
                        message: format!(
                            "#include with absolute path rejected for security: {}",
                            include_path
                        ),
                    });
                }

                let full_path = base_dir.join(include_path);

                let include_content =
                    std::fs::read_to_string(&full_path).map_err(|_| {
                        ConfigError::IncludeNotFound(full_path.display().to_string())
                    })?;

                let include_dir = full_path.parent().unwrap_or(base_dir);
                let include_filename = full_path.display().to_string();

                Parser::parse_content_inner(
                    &include_content,
                    &include_filename,
                    include_dir,
                    config,
                    depth + 1,
                )?;

                continue;
            }

            // Handle #exec directives (log as unsupported, skip)
            if line.starts_with("#exec") {
                continue;
            }

            // Handle category headers [name], [name](!), [name](template)
            if line.starts_with('[') {
                // Save previous category
                if let Some(cat) = current_category.take() {
                    config.categories.push(cat);
                }

                let (name, is_template, template_name) =
                    parse_category_header(line, filename, lineno)?;

                current_category = Some(Category {
                    name,
                    is_template,
                    variables: Vec::new(),
                    template_name,
                    file: filename.to_string(),
                    lineno,
                });

                continue;
            }

            // Handle variable assignments: key = value or key => value
            if let Some(cat) = current_category.as_mut() {
                let (name, value, is_object) = parse_variable(line, filename, lineno)?;
                cat.variables.push(Variable {
                    name,
                    value,
                    file: filename.to_string(),
                    lineno,
                    is_object,
                    inherited: false,
                });
            }
            // Variable outside any category -- skip it (matching Asterisk behavior)
        }

        // Save last category
        if let Some(cat) = current_category {
            config.categories.push(cat);
        }

        Ok(())
    }
}

/// Strip inline comments. A semicolon (;) that is not inside quotes ends the line.
fn strip_inline_comment(line: &str) -> &str {
    let mut in_quotes = false;
    for (i, ch) in line.char_indices() {
        if ch == '"' {
            in_quotes = !in_quotes;
        } else if ch == ';' && !in_quotes {
            return &line[..i];
        }
    }
    line
}

/// Parse a category header like `[name]`, `[name](!)`, `[name](template)`,
/// or `[name](template1,template2)`.
fn parse_category_header(
    line: &str,
    filename: &str,
    lineno: usize,
) -> Result<(String, bool, Option<String>), ConfigError> {
    let line = line.trim();

    // Find the closing bracket
    let close_bracket = line.find(']').ok_or_else(|| ConfigError::ParseError {
        file: filename.to_string(),
        line: lineno,
        message: format!("missing closing ']' in category header: {}", line),
    })?;

    let name = line[1..close_bracket].trim().to_string();

    if name.is_empty() {
        return Err(ConfigError::ParseError {
            file: filename.to_string(),
            line: lineno,
            message: "empty category name".to_string(),
        });
    }

    let after_bracket = line[close_bracket + 1..].trim();

    let mut is_template = false;
    let mut template_name = None;

    // Check for (!) or (template_name) after the bracket
    if let Some(rest) = after_bracket.strip_prefix('(') {
        let close_paren = rest.find(')').ok_or_else(|| ConfigError::ParseError {
            file: filename.to_string(),
            line: lineno,
            message: "missing closing ')' in category modifier".to_string(),
        })?;

        let modifier = rest[..close_paren].trim();

        if modifier == "!" {
            is_template = true;
        } else if !modifier.is_empty() {
            // Could be a comma-separated list of templates; take the first one
            let first_template = modifier.split(',').next().unwrap().trim();
            if first_template == "!" {
                is_template = true;
            } else {
                template_name = Some(first_template.to_string());
            }
            // Check if there is also a '!' in the list
            if modifier.split(',').any(|s| s.trim() == "!") {
                is_template = true;
            }
        }
    }

    Ok((name, is_template, template_name))
}

/// Parse a variable line like `key = value` or `key => value`.
fn parse_variable(
    line: &str,
    filename: &str,
    lineno: usize,
) -> Result<(String, String, bool), ConfigError> {
    // Try => first (object assignment)
    if let Some(pos) = line.find("=>") {
        let name = line[..pos].trim().to_string();
        let value = line[pos + 2..].trim().to_string();
        if name.is_empty() {
            return Err(ConfigError::ParseError {
                file: filename.to_string(),
                line: lineno,
                message: "empty variable name".to_string(),
            });
        }
        return Ok((name, value, true));
    }

    // Try = (regular assignment)
    if let Some(pos) = line.find('=') {
        let name = line[..pos].trim().to_string();
        let value = line[pos + 1..].trim().to_string();
        if name.is_empty() {
            return Err(ConfigError::ParseError {
                file: filename.to_string(),
                line: lineno,
                message: "empty variable name".to_string(),
            });
        }
        return Ok((name, value, false));
    }

    Err(ConfigError::ParseError {
        file: filename.to_string(),
        line: lineno,
        message: format!("could not parse line as variable assignment: {}", line),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_config() {
        let content = r#"
; This is a comment
[general]
context = default
allowguest = no
bindport = 5060

[my-phone](!)
type = friend
host = dynamic

[phone1](my-phone)
secret = password123
callerid = "Phone 1" <100>
"#;
        let config = AsteriskConfig::from_str(content, "test.conf").unwrap();

        assert_eq!(config.categories.len(), 3);
        assert_eq!(config.get_variable("general", "context"), Some("default"));
        assert_eq!(config.get_variable("general", "allowguest"), Some("no"));
        assert_eq!(config.get_variable("general", "bindport"), Some("5060"));

        let my_phone = config.get_category("my-phone").unwrap();
        assert!(my_phone.is_template);
        assert_eq!(my_phone.get_variable("type"), Some("friend"));

        let phone1 = config.get_category("phone1").unwrap();
        assert!(!phone1.is_template);
        assert_eq!(phone1.template_name.as_deref(), Some("my-phone"));
        // Should have inherited variables from template
        assert_eq!(phone1.get_variable("type"), Some("friend"));
        assert_eq!(phone1.get_variable("secret"), Some("password123"));
    }

    #[test]
    fn test_object_assignment() {
        let content = r#"
[extensions]
exten => 100,1,Answer()
exten => 100,2,Hangup()
"#;
        let config = AsteriskConfig::from_str(content, "test.conf").unwrap();
        let ext = config.get_category("extensions").unwrap();
        assert_eq!(ext.variables.len(), 2);
        assert!(ext.variables[0].is_object);
        assert_eq!(ext.variables[0].name, "exten");
    }

    #[test]
    fn test_inline_comments() {
        let content = r#"
[general]
context = default ; this is a comment
secret = "pass;word" ; semicolons in quotes are preserved
"#;
        let config = AsteriskConfig::from_str(content, "test.conf").unwrap();
        assert_eq!(config.get_variable("general", "context"), Some("default"));
        assert_eq!(
            config.get_variable("general", "secret"),
            Some("\"pass;word\"")
        );
    }

    #[test]
    fn test_category_names() {
        let content = r#"
[general]
key = val

[peers]
key = val
"#;
        let config = AsteriskConfig::from_str(content, "test.conf").unwrap();
        let names = config.category_names();
        assert_eq!(names, vec!["general", "peers"]);
    }
}
