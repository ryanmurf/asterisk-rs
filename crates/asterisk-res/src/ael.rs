//! AEL (Asterisk Extension Language) dialplan parser.
//!
//! Port of `res/res_ael2.c` and `pbx/pbx_ael.c`. Provides a tokenizer
//! and AST for the AEL syntax, which is a structured alternative to the
//! extensions.conf dialplan format. AEL uses C-like syntax with contexts,
//! extensions, macros, and control flow (if/while/switch).

use std::fmt;

use thiserror::Error;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum AelError {
    #[error("parse error at line {line}: {message}")]
    ParseError { line: u32, message: String },
    #[error("unexpected token: expected {expected}, got {got}")]
    UnexpectedToken { expected: String, got: String },
    #[error("AEL error: {0}")]
    Other(String),
}

pub type AelResult<T> = Result<T, AelError>;

// ---------------------------------------------------------------------------
// Token types
// ---------------------------------------------------------------------------

/// AEL token types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AelToken {
    /// `context` keyword.
    Context,
    /// `macro` keyword.
    Macro,
    /// `globals` keyword.
    Globals,
    /// `if` keyword.
    If,
    /// `else` keyword.
    Else,
    /// `while` keyword.
    While,
    /// `for` keyword.
    For,
    /// `switch` keyword.
    Switch,
    /// `case` keyword.
    Case,
    /// `default` keyword.
    Default,
    /// `pattern` keyword.
    Pattern,
    /// `break` keyword.
    Break,
    /// `continue` keyword.
    Continue,
    /// `return` keyword.
    Return,
    /// `goto` keyword.
    Goto,
    /// `{`
    LBrace,
    /// `}`
    RBrace,
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `;`
    Semicolon,
    /// `=>`
    Arrow,
    /// `=`
    Assign,
    /// `,`
    Comma,
    /// `&` (background application execution).
    Ampersand,
    /// `|` (pipe / catch).
    Pipe,
    /// An identifier or word.
    Ident(String),
    /// A string literal.
    StringLit(String),
    /// A numeric literal.
    Number(String),
    /// End of file.
    Eof,
}

// ---------------------------------------------------------------------------
// AST node types
// ---------------------------------------------------------------------------

/// An AEL abstract syntax tree node.
#[derive(Debug, Clone)]
pub enum AelNode {
    /// Top-level file containing contexts, macros, and globals.
    File {
        contexts: Vec<AelNode>,
        macros: Vec<AelNode>,
        globals: Vec<(String, String)>,
    },
    /// `context name { ... }`
    Context {
        name: String,
        extensions: Vec<AelNode>,
    },
    /// `exten => priority,app(args)`
    Extension {
        pattern: String,
        statements: Vec<AelNode>,
    },
    /// Application call: `App(args)`
    Application {
        name: String,
        args: String,
    },
    /// `if (condition) { ... } else { ... }`
    IfElse {
        condition: String,
        if_body: Vec<AelNode>,
        else_body: Vec<AelNode>,
    },
    /// `while (condition) { ... }`
    While {
        condition: String,
        body: Vec<AelNode>,
    },
    /// `switch (expression) { case val: ... }`
    Switch {
        expression: String,
        cases: Vec<(String, Vec<AelNode>)>,
        default: Vec<AelNode>,
    },
    /// `macro name(args) { ... }`
    MacroDef {
        name: String,
        args: Vec<String>,
        body: Vec<AelNode>,
    },
    /// `&macro_name(args)` -- macro call.
    MacroCall {
        name: String,
        args: String,
    },
    /// `goto context,extension,priority`
    Goto {
        context: Option<String>,
        extension: String,
        priority: String,
    },
    /// Variable assignment: `Set(name=value)`
    Assignment {
        variable: String,
        value: String,
    },
}

impl fmt::Display for AelNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Context { name, extensions } => {
                write!(f, "context {} ({} extensions)", name, extensions.len())
            }
            Self::Extension { pattern, .. } => write!(f, "exten => {}", pattern),
            Self::Application { name, args } => write!(f, "{}({})", name, args),
            _ => write!(f, "{:?}", std::mem::discriminant(self)),
        }
    }
}

// ---------------------------------------------------------------------------
// Simple tokenizer
// ---------------------------------------------------------------------------

/// Classify an AEL keyword.
pub fn classify_keyword(word: &str) -> AelToken {
    match word {
        "context" => AelToken::Context,
        "macro" => AelToken::Macro,
        "globals" => AelToken::Globals,
        "if" => AelToken::If,
        "else" => AelToken::Else,
        "while" => AelToken::While,
        "for" => AelToken::For,
        "switch" => AelToken::Switch,
        "case" => AelToken::Case,
        "default" => AelToken::Default,
        "pattern" => AelToken::Pattern,
        "break" => AelToken::Break,
        "continue" => AelToken::Continue,
        "return" => AelToken::Return,
        "goto" => AelToken::Goto,
        _ => AelToken::Ident(word.to_string()),
    }
}

/// Tokenize a single line of AEL source (simplified).
pub fn tokenize_line(line: &str) -> Vec<AelToken> {
    let mut tokens = Vec::new();
    let mut chars = line.chars().peekable();
    let mut word = String::new();

    while let Some(&ch) = chars.peek() {
        match ch {
            ' ' | '\t' | '\r' | '\n' => {
                if !word.is_empty() {
                    tokens.push(classify_keyword(&word));
                    word.clear();
                }
                chars.next();
            }
            '{' => {
                if !word.is_empty() {
                    tokens.push(classify_keyword(&word));
                    word.clear();
                }
                tokens.push(AelToken::LBrace);
                chars.next();
            }
            '}' => {
                if !word.is_empty() {
                    tokens.push(classify_keyword(&word));
                    word.clear();
                }
                tokens.push(AelToken::RBrace);
                chars.next();
            }
            '(' => {
                if !word.is_empty() {
                    tokens.push(classify_keyword(&word));
                    word.clear();
                }
                tokens.push(AelToken::LParen);
                chars.next();
            }
            ')' => {
                if !word.is_empty() {
                    tokens.push(classify_keyword(&word));
                    word.clear();
                }
                tokens.push(AelToken::RParen);
                chars.next();
            }
            ';' => {
                if !word.is_empty() {
                    tokens.push(classify_keyword(&word));
                    word.clear();
                }
                tokens.push(AelToken::Semicolon);
                chars.next();
            }
            '=' => {
                if !word.is_empty() {
                    tokens.push(classify_keyword(&word));
                    word.clear();
                }
                chars.next();
                if chars.peek() == Some(&'>') {
                    chars.next();
                    tokens.push(AelToken::Arrow);
                } else {
                    tokens.push(AelToken::Assign);
                }
            }
            ',' => {
                if !word.is_empty() {
                    tokens.push(classify_keyword(&word));
                    word.clear();
                }
                tokens.push(AelToken::Comma);
                chars.next();
            }
            '&' => {
                if !word.is_empty() {
                    tokens.push(classify_keyword(&word));
                    word.clear();
                }
                tokens.push(AelToken::Ampersand);
                chars.next();
            }
            '/' if chars.clone().nth(1) == Some('/') => {
                // Line comment: stop tokenizing this line.
                break;
            }
            _ => {
                word.push(ch);
                chars.next();
            }
        }
    }

    if !word.is_empty() {
        tokens.push(classify_keyword(&word));
    }

    tokens
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_keywords() {
        assert_eq!(classify_keyword("context"), AelToken::Context);
        assert_eq!(classify_keyword("if"), AelToken::If);
        assert_eq!(classify_keyword("myapp"), AelToken::Ident("myapp".into()));
    }

    #[test]
    fn test_tokenize_context() {
        let tokens = tokenize_line("context default {");
        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[0], AelToken::Context);
        // "default" is a keyword, so it tokenizes as AelToken::Default,
        // not AelToken::Ident.  In AEL, `context default { ... }` is valid
        // and uses the keyword "default" as the context name.
        assert_eq!(tokens[1], AelToken::Default);
        assert_eq!(tokens[2], AelToken::LBrace);
    }

    #[test]
    fn test_tokenize_extension() {
        let tokens = tokenize_line("s => {");
        assert_eq!(tokens[0], AelToken::Ident("s".into()));
        assert_eq!(tokens[1], AelToken::Arrow);
        assert_eq!(tokens[2], AelToken::LBrace);
    }

    #[test]
    fn test_tokenize_app_call() {
        let tokens = tokenize_line("Answer();");
        assert!(tokens.contains(&AelToken::LParen));
        assert!(tokens.contains(&AelToken::RParen));
        assert!(tokens.contains(&AelToken::Semicolon));
    }

    #[test]
    fn test_tokenize_comment() {
        let tokens = tokenize_line("Answer(); // answer the call");
        // Comment is stripped, so only tokens before it
        assert!(tokens.contains(&AelToken::Semicolon));
        assert!(!tokens.iter().any(|t| matches!(t, AelToken::Ident(s) if s.contains("answer"))));
    }

    #[test]
    fn test_ael_node_display() {
        let node = AelNode::Context {
            name: "default".into(),
            extensions: vec![],
        };
        assert!(format!("{}", node).contains("default"));
    }
}
