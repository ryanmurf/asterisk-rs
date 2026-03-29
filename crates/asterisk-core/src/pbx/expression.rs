//! Expression evaluator for `$[...]` expressions in Asterisk dialplan.
//!
//! This evaluates expressions used in GotoIf, ExecIf, While, etc.
//! Ported from the C `ast_expr2.c` / `ast_expr2.y` Bison grammar.
//!
//! Operator precedence (lowest to highest):
//!   `|`  (logical OR)
//!   `&`  (logical AND)
//!   `=` `!=` `<` `>` `<=` `>=`  (comparison)
//!   `+` `-`  (addition/subtraction)
//!   `*` `/` `%`  (multiplication/division/modulo)
//!   `!`  (logical NOT, unary)
//!   `-` (unary negation)
//!   `=~` `:`  (regex match)
//!   `(` `)` (grouping)
//!   `? :`  (ternary conditional)

use std::fmt;

/// Errors that can occur during expression evaluation.
#[derive(Debug, Clone)]
pub enum ExprError {
    /// Unexpected end of expression.
    UnexpectedEnd,
    /// Unexpected token encountered.
    UnexpectedToken(String),
    /// Division by zero.
    DivisionByZero,
    /// Invalid regex pattern.
    InvalidRegex(String),
    /// General parse error.
    ParseError(String),
}

impl fmt::Display for ExprError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEnd => write!(f, "unexpected end of expression"),
            Self::UnexpectedToken(t) => write!(f, "unexpected token: {}", t),
            Self::DivisionByZero => write!(f, "division by zero"),
            Self::InvalidRegex(r) => write!(f, "invalid regex: {}", r),
            Self::ParseError(msg) => write!(f, "parse error: {}", msg),
        }
    }
}

impl std::error::Error for ExprError {}

/// An expression value -- either a string or a number (f64).
///
/// Mirrors the C `struct val` with `enum valtype { AST_EXPR_number, AST_EXPR_numeric_string, AST_EXPR_string }`.
#[derive(Debug, Clone)]
enum Value {
    Number(f64),
    Str(String),
}

impl Value {
    /// Try to interpret this value as a number.
    fn as_number(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            Value::Str(s) => {
                let s = s.trim();
                if s.is_empty() {
                    return None;
                }
                s.parse::<f64>().ok()
            }
        }
    }

    /// Convert to string representation.
    fn as_string(&self) -> String {
        match self {
            Value::Number(n) => format_number(*n),
            Value::Str(s) => s.clone(),
        }
    }

    /// Check if the value is "zero or null" (falsy) like the C `is_zero_or_null`.
    fn is_zero_or_null(&self) -> bool {
        match self {
            Value::Number(n) => *n == 0.0,
            Value::Str(s) => {
                let s = s.trim();
                if s.is_empty() {
                    return true;
                }
                if let Ok(n) = s.parse::<f64>() {
                    n == 0.0
                } else {
                    // Non-numeric, non-empty string is truthy in Asterisk
                    false
                }
            }
        }
    }

    /// Force to number; returns 0 if not numeric.
    fn to_number(&self) -> f64 {
        self.as_number().unwrap_or(0.0)
    }

    /// Check if both values can be compared numerically.
    fn both_numeric(a: &Value, b: &Value) -> bool {
        a.as_number().is_some() && b.as_number().is_some()
    }
}

/// Format a number the way Asterisk does -- integers without decimal point.
///
/// Uses saturating conversion for values outside i64 range to avoid
/// undefined behavior on overflow.
fn format_number(n: f64) -> String {
    if n == n.trunc() && n.is_finite() {
        // Integer value -- print without decimal point.
        // Saturate to i64::MIN/MAX to avoid UB on out-of-range f64->i64 cast.
        let clamped = n.clamp(i64::MIN as f64, i64::MAX as f64);
        format!("{}", clamped as i64)
    } else {
        format!("{}", n)
    }
}

/// Token types for the expression lexer.
#[derive(Debug, Clone, PartialEq)]
enum Token {
    /// A literal value (number or string).
    Value(String),
    /// `|`
    Or,
    /// `&`
    And,
    /// `=`
    Eq,
    /// `!=`
    Ne,
    /// `<`
    Lt,
    /// `>`
    Gt,
    /// `<=`
    Le,
    /// `>=`
    Ge,
    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `*`
    Mult,
    /// `/`
    Div,
    /// `%`
    Mod,
    /// `!`
    Not,
    /// `=~`
    EqTilde,
    /// `:`
    Colon,
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `?`
    Question,
    /// End of input.
    Eof,
}

/// Tokenizer for expression strings.
struct Lexer {
    chars: Vec<char>,
    pos: usize,
}

impl Lexer {
    fn new(input: &str) -> Self {
        Self {
            chars: input.chars().collect(),
            pos: 0,
        }
    }

    fn peek_char(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn next_char(&mut self) -> Option<char> {
        let c = self.chars.get(self.pos).copied();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn skip_whitespace(&mut self) {
        while let Some(c) = self.peek_char() {
            if c.is_whitespace() {
                self.next_char();
            } else {
                break;
            }
        }
    }

    fn next_token(&mut self) -> Result<Token, ExprError> {
        self.skip_whitespace();

        let c = match self.peek_char() {
            None => return Ok(Token::Eof),
            Some(c) => c,
        };

        match c {
            '(' => {
                self.next_char();
                Ok(Token::LParen)
            }
            ')' => {
                self.next_char();
                Ok(Token::RParen)
            }
            '|' => {
                self.next_char();
                Ok(Token::Or)
            }
            '&' => {
                self.next_char();
                Ok(Token::And)
            }
            '+' => {
                self.next_char();
                Ok(Token::Plus)
            }
            '-' => {
                self.next_char();
                Ok(Token::Minus)
            }
            '*' => {
                self.next_char();
                Ok(Token::Mult)
            }
            '/' => {
                self.next_char();
                Ok(Token::Div)
            }
            '%' => {
                self.next_char();
                Ok(Token::Mod)
            }
            '?' => {
                self.next_char();
                Ok(Token::Question)
            }
            ':' => {
                self.next_char();
                Ok(Token::Colon)
            }
            '!' => {
                self.next_char();
                if self.peek_char() == Some('=') {
                    self.next_char();
                    Ok(Token::Ne)
                } else {
                    Ok(Token::Not)
                }
            }
            '=' => {
                self.next_char();
                if self.peek_char() == Some('~') {
                    self.next_char();
                    Ok(Token::EqTilde)
                } else {
                    Ok(Token::Eq)
                }
            }
            '<' => {
                self.next_char();
                if self.peek_char() == Some('=') {
                    self.next_char();
                    Ok(Token::Le)
                } else {
                    Ok(Token::Lt)
                }
            }
            '>' => {
                self.next_char();
                if self.peek_char() == Some('=') {
                    self.next_char();
                    Ok(Token::Ge)
                } else {
                    Ok(Token::Gt)
                }
            }
            '"' => {
                // Quoted string
                self.next_char(); // consume opening quote
                let mut s = String::new();
                loop {
                    match self.next_char() {
                        None => break, // unterminated string
                        Some('"') => break,
                        Some('\\') => {
                            // Escape next character
                            if let Some(esc) = self.next_char() {
                                s.push(esc);
                            }
                        }
                        Some(ch) => s.push(ch),
                    }
                }
                Ok(Token::Value(s))
            }
            _ => {
                // Unquoted token -- collect until whitespace or operator
                let mut s = String::new();
                while let Some(ch) = self.peek_char() {
                    match ch {
                        '(' | ')' | '|' | '&' | '+' | '*' | '/' | '%' | '?' | ':' | '!' | '='
                        | '<' | '>' | '"' => break,
                        '-' if !s.is_empty() => break,
                        c if c.is_whitespace() => break,
                        _ => {
                            s.push(ch);
                            self.next_char();
                        }
                    }
                }
                if s.is_empty() {
                    Err(ExprError::UnexpectedToken(format!("{}", c)))
                } else {
                    Ok(Token::Value(s))
                }
            }
        }
    }
}

/// Recursive descent parser and evaluator for Asterisk expressions.
///
/// Grammar (from lowest to highest precedence):
/// ```text
/// expr       := ternary
/// ternary    := or_expr ('?' ternary ':' ternary)?
/// or_expr    := and_expr ('|' and_expr)*
/// and_expr   := cmp_expr ('&' cmp_expr)*
/// cmp_expr   := add_expr (('=' | '!=' | '<' | '>' | '<=' | '>=') add_expr)*
/// add_expr   := mul_expr (('+' | '-') mul_expr)*
/// mul_expr   := regex_expr (('*' | '/' | '%') regex_expr)*
/// regex_expr := unary_expr (('=~' | ':') unary_expr)?
/// unary_expr := '!' unary_expr | '-' unary_expr | primary
/// primary    := '(' expr ')' | VALUE
/// ```
struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    /// Tracks ternary nesting depth so that `:` at the regex level
    /// is not consumed when we are parsing a ternary branch.
    ternary_depth: usize,
}

impl Parser {
    fn new(input: &str) -> Result<Self, ExprError> {
        let mut lexer = Lexer::new(input);
        let mut tokens = Vec::new();
        loop {
            let tok = lexer.next_token()?;
            let is_eof = tok == Token::Eof;
            tokens.push(tok);
            if is_eof {
                break;
            }
        }
        Ok(Self {
            tokens,
            pos: 0,
            ternary_depth: 0,
        })
    }

    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&Token::Eof)
    }

    fn advance(&mut self) -> Token {
        let tok = self.tokens.get(self.pos).cloned().unwrap_or(Token::Eof);
        self.pos += 1;
        tok
    }

    fn expect(&mut self, expected: &Token) -> Result<(), ExprError> {
        let tok = self.advance();
        if &tok == expected {
            Ok(())
        } else {
            Err(ExprError::UnexpectedToken(format!(
                "expected {:?}, got {:?}",
                expected, tok
            )))
        }
    }

    fn parse_expr(&mut self) -> Result<Value, ExprError> {
        self.parse_ternary()
    }

    fn parse_ternary(&mut self) -> Result<Value, ExprError> {
        let cond = self.parse_or()?;

        if *self.peek() == Token::Question {
            self.advance(); // consume '?'
            self.ternary_depth += 1;
            let true_val = self.parse_ternary()?;
            self.expect(&Token::Colon)?;
            let false_val = self.parse_ternary()?;
            self.ternary_depth -= 1;

            if !cond.is_zero_or_null() {
                Ok(true_val)
            } else {
                Ok(false_val)
            }
        } else {
            Ok(cond)
        }
    }

    fn parse_or(&mut self) -> Result<Value, ExprError> {
        let mut left = self.parse_and()?;

        while *self.peek() == Token::Or {
            self.advance();
            let right = self.parse_and()?;
            // Asterisk OR: if left is non-zero/non-null, return left, else return right
            if !left.is_zero_or_null() {
                // left is truthy, result is left
            } else if !right.is_zero_or_null() {
                left = right;
            } else {
                left = Value::Number(0.0);
            }
        }

        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Value, ExprError> {
        let mut left = self.parse_comparison()?;

        while *self.peek() == Token::And {
            self.advance();
            let right = self.parse_comparison()?;
            // Asterisk AND: if both are non-zero/non-null, return left, else 0
            if !left.is_zero_or_null() && !right.is_zero_or_null() {
                // result is left
            } else {
                left = Value::Number(0.0);
            }
        }

        Ok(left)
    }

    fn parse_comparison(&mut self) -> Result<Value, ExprError> {
        let mut left = self.parse_addition()?;

        loop {
            let op = match self.peek() {
                Token::Eq => Token::Eq,
                Token::Ne => Token::Ne,
                Token::Lt => Token::Lt,
                Token::Gt => Token::Gt,
                Token::Le => Token::Le,
                Token::Ge => Token::Ge,
                _ => break,
            };
            self.advance();
            let right = self.parse_addition()?;

            let result = if Value::both_numeric(&left, &right) {
                let l = left.to_number();
                let r = right.to_number();
                match op {
                    Token::Eq => l == r,
                    Token::Ne => l != r,
                    Token::Lt => l < r,
                    Token::Gt => l > r,
                    Token::Le => l <= r,
                    Token::Ge => l >= r,
                    _ => unreachable!(),
                }
            } else {
                let l = left.as_string();
                let r = right.as_string();
                match op {
                    Token::Eq => l == r,
                    Token::Ne => l != r,
                    Token::Lt => l < r,
                    Token::Gt => l > r,
                    Token::Le => l <= r,
                    Token::Ge => l >= r,
                    _ => unreachable!(),
                }
            };

            left = Value::Number(if result { 1.0 } else { 0.0 });
        }

        Ok(left)
    }

    fn parse_addition(&mut self) -> Result<Value, ExprError> {
        let mut left = self.parse_multiplication()?;

        loop {
            let op = match self.peek() {
                Token::Plus => Token::Plus,
                Token::Minus => Token::Minus,
                _ => break,
            };
            self.advance();
            let right = self.parse_multiplication()?;

            let l = left.to_number();
            let r = right.to_number();
            left = Value::Number(match op {
                Token::Plus => l + r,
                Token::Minus => l - r,
                _ => unreachable!(),
            });
        }

        Ok(left)
    }

    fn parse_multiplication(&mut self) -> Result<Value, ExprError> {
        let mut left = self.parse_regex()?;

        loop {
            let op = match self.peek() {
                Token::Mult => Token::Mult,
                Token::Div => Token::Div,
                Token::Mod => Token::Mod,
                _ => break,
            };
            self.advance();
            let right = self.parse_regex()?;

            let l = left.to_number();
            let r = right.to_number();

            if matches!(op, Token::Div | Token::Mod) && r == 0.0 {
                return Err(ExprError::DivisionByZero);
            }

            left = Value::Number(match op {
                Token::Mult => l * r,
                Token::Div => {
                    // Integer division when both are integers
                    if l == l.trunc() && r == r.trunc() {
                        (l as i64 / r as i64) as f64
                    } else {
                        l / r
                    }
                }
                Token::Mod => {
                    if l == l.trunc() && r == r.trunc() {
                        (l as i64 % r as i64) as f64
                    } else {
                        l % r
                    }
                }
                _ => unreachable!(),
            });
        }

        Ok(left)
    }

    fn parse_regex(&mut self) -> Result<Value, ExprError> {
        let left = self.parse_unary()?;

        let op = match self.peek() {
            Token::EqTilde => Token::EqTilde,
            // Only consume ':' as regex operator when NOT inside a ternary.
            // Inside a ternary, ':' separates the true/false branches.
            Token::Colon if self.ternary_depth == 0 => Token::Colon,
            _ => return Ok(left),
        };
        self.advance();
        let right = self.parse_unary()?;

        let string = left.as_string();
        let pattern = right.as_string();

        // For ':' operator, the pattern is implicitly anchored at the start
        let pattern_str = if op == Token::Colon {
            format!("^(?:{})", pattern)
        } else {
            pattern.clone()
        };

        let re = regex::Regex::new(&pattern_str)
            .map_err(|e| ExprError::InvalidRegex(format!("{}: {}", pattern, e)))?;

        if let Some(captures) = re.captures(&string) {
            if captures.len() > 1 {
                // Has a capture group -- return the captured text
                let matched = captures.get(1).map(|m| m.as_str()).unwrap_or("");
                Ok(Value::Str(matched.to_string()))
            } else {
                // No capture group -- return length of overall match
                let m = captures.get(0).unwrap();
                Ok(Value::Number(m.as_str().len() as f64))
            }
        } else {
            // No match
            // If the pattern has capture groups, return empty string; otherwise return 0
            if pattern.contains('(') && !pattern.starts_with("\\(") {
                Ok(Value::Str(String::new()))
            } else {
                Ok(Value::Number(0.0))
            }
        }
    }

    fn parse_unary(&mut self) -> Result<Value, ExprError> {
        match self.peek() {
            Token::Not => {
                self.advance();
                let val = self.parse_unary()?;
                Ok(Value::Number(if val.is_zero_or_null() {
                    1.0
                } else {
                    0.0
                }))
            }
            Token::Minus => {
                self.advance();
                // Only treat as unary negation if the next token is a value or paren
                let val = self.parse_unary()?;
                Ok(Value::Number(-val.to_number()))
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Result<Value, ExprError> {
        match self.peek().clone() {
            Token::LParen => {
                self.advance();
                let val = self.parse_expr()?;
                self.expect(&Token::RParen)?;
                Ok(val)
            }
            Token::Value(_) => {
                if let Token::Value(s) = self.advance() {
                    // Try to parse as number
                    if let Ok(n) = s.parse::<f64>() {
                        Ok(Value::Number(n))
                    } else {
                        Ok(Value::Str(s))
                    }
                } else {
                    unreachable!()
                }
            }
            Token::Eof => Err(ExprError::UnexpectedEnd),
            other => Err(ExprError::UnexpectedToken(format!("{:?}", other))),
        }
    }
}

/// Evaluate an Asterisk expression string.
///
/// This handles expressions like `$[1 + 2]` -- the `$[` and `]` wrappers
/// should already be stripped; pass just the inner expression (e.g. `"1 + 2"`).
///
/// Returns the result as a string, matching Asterisk's behavior where everything
/// is ultimately a string.
pub fn evaluate_expression(expr: &str) -> Result<String, ExprError> {
    let expr = expr.trim();
    if expr.is_empty() {
        return Ok("0".to_string());
    }

    let mut parser = Parser::new(expr)?;
    let result = parser.parse_expr()?;

    // Check that we consumed all tokens
    if *parser.peek() != Token::Eof {
        return Err(ExprError::UnexpectedToken(format!(
            "trailing tokens after expression: {:?}",
            parser.peek()
        )));
    }

    Ok(result.as_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_arithmetic() {
        assert_eq!(evaluate_expression("1 + 2").unwrap(), "3");
        assert_eq!(evaluate_expression("10 - 3").unwrap(), "7");
        assert_eq!(evaluate_expression("4 * 5").unwrap(), "20");
        assert_eq!(evaluate_expression("10 / 3").unwrap(), "3");
        assert_eq!(evaluate_expression("10 % 3").unwrap(), "1");
    }

    #[test]
    fn test_negative_numbers() {
        assert_eq!(evaluate_expression("-5").unwrap(), "-5");
        assert_eq!(evaluate_expression("-5 + 3").unwrap(), "-2");
        assert_eq!(evaluate_expression("5 + -3").unwrap(), "2");
    }

    #[test]
    fn test_parentheses() {
        assert_eq!(evaluate_expression("(1 + 2) * 3").unwrap(), "9");
        assert_eq!(evaluate_expression("2 * (3 + 4)").unwrap(), "14");
        assert_eq!(evaluate_expression("((1 + 2))").unwrap(), "3");
    }

    #[test]
    fn test_comparison_numeric() {
        assert_eq!(evaluate_expression("5 > 3").unwrap(), "1");
        assert_eq!(evaluate_expression("3 > 5").unwrap(), "0");
        assert_eq!(evaluate_expression("5 = 5").unwrap(), "1");
        assert_eq!(evaluate_expression("5 != 3").unwrap(), "1");
        assert_eq!(evaluate_expression("5 != 5").unwrap(), "0");
        assert_eq!(evaluate_expression("3 < 5").unwrap(), "1");
        assert_eq!(evaluate_expression("5 <= 5").unwrap(), "1");
        assert_eq!(evaluate_expression("5 >= 5").unwrap(), "1");
        assert_eq!(evaluate_expression("5 >= 6").unwrap(), "0");
    }

    #[test]
    fn test_comparison_string() {
        assert_eq!(evaluate_expression(r#""abc" = "abc""#).unwrap(), "1");
        assert_eq!(evaluate_expression(r#""abc" != "def""#).unwrap(), "1");
        assert_eq!(evaluate_expression(r#""abc" < "def""#).unwrap(), "1");
        assert_eq!(evaluate_expression(r#""def" > "abc""#).unwrap(), "1");
    }

    #[test]
    fn test_logical_operators() {
        // AND: both truthy -> returns left; either falsy -> 0
        assert_eq!(evaluate_expression("1 & 2").unwrap(), "1");
        assert_eq!(evaluate_expression("0 & 2").unwrap(), "0");
        assert_eq!(evaluate_expression("1 & 0").unwrap(), "0");

        // OR: left truthy -> returns left; right truthy -> returns right
        assert_eq!(evaluate_expression("1 | 2").unwrap(), "1");
        assert_eq!(evaluate_expression("0 | 2").unwrap(), "2");
        assert_eq!(evaluate_expression("0 | 0").unwrap(), "0");
    }

    #[test]
    fn test_not() {
        assert_eq!(evaluate_expression("!0").unwrap(), "1");
        assert_eq!(evaluate_expression("!1").unwrap(), "0");
        assert_eq!(evaluate_expression("!5").unwrap(), "0");
    }

    #[test]
    fn test_ternary() {
        assert_eq!(evaluate_expression("1 ? 10 : 20").unwrap(), "10");
        assert_eq!(evaluate_expression("0 ? 10 : 20").unwrap(), "20");
        assert_eq!(evaluate_expression("5 > 3 ? 100 : 200").unwrap(), "100");
    }

    #[test]
    fn test_regex_colon() {
        // ':' operator - anchored at start, returns match length or captured group
        assert_eq!(evaluate_expression(r#""abcdef" : "abc""#).unwrap(), "3");
        assert_eq!(evaluate_expression(r#""abcdef" : "xyz""#).unwrap(), "0");
    }

    #[test]
    fn test_regex_eqtilde() {
        // '=~' operator - not anchored
        assert_eq!(evaluate_expression(r#""abcdef" =~ "cd""#).unwrap(), "2");
        assert_eq!(evaluate_expression(r#""abcdef" =~ "xyz""#).unwrap(), "0");
    }

    #[test]
    fn test_regex_capture() {
        assert_eq!(
            evaluate_expression(r#""abc123" : "abc(...)""#).unwrap(),
            "123"
        );
    }

    #[test]
    fn test_operator_precedence() {
        // * before +
        assert_eq!(evaluate_expression("2 + 3 * 4").unwrap(), "14");
        // Parentheses override
        assert_eq!(evaluate_expression("(2 + 3) * 4").unwrap(), "20");
    }

    #[test]
    fn test_division_by_zero() {
        assert!(evaluate_expression("1 / 0").is_err());
        assert!(evaluate_expression("1 % 0").is_err());
    }

    #[test]
    fn test_empty_expression() {
        assert_eq!(evaluate_expression("").unwrap(), "0");
        assert_eq!(evaluate_expression("  ").unwrap(), "0");
    }

    #[test]
    fn test_complex_expression() {
        // Mimics: $[${COUNT} > 5 & ${COUNT} < 10]
        // With values substituted to: $[7 > 5 & 7 < 10]
        assert_eq!(evaluate_expression("7 > 5 & 7 < 10").unwrap(), "1");
        assert_eq!(evaluate_expression("3 > 5 & 3 < 10").unwrap(), "0");
    }

    #[test]
    fn test_nested_ternary() {
        assert_eq!(
            evaluate_expression("1 ? 2 ? 10 : 20 : 30").unwrap(),
            "10"
        );
        assert_eq!(
            evaluate_expression("1 ? 0 ? 10 : 20 : 30").unwrap(),
            "20"
        );
        assert_eq!(
            evaluate_expression("0 ? 10 : 1 ? 20 : 30").unwrap(),
            "20"
        );
    }

    #[test]
    fn test_string_values() {
        // Non-numeric strings
        assert_eq!(evaluate_expression(r#""hello""#).unwrap(), "hello");
    }

    #[test]
    fn test_large_numbers() {
        assert_eq!(evaluate_expression("1000000 * 1000000").unwrap(), "1000000000000");
    }
}
