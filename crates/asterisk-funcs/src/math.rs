//! MATH() function - basic arithmetic expression evaluator.
//!
//! Port of func_math.c from Asterisk C.

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// MATH() function.
///
/// Performs mathematical operations.
///
/// Usage: MATH(number1 op number2[,type])
///
/// Supported operators: +, -, *, /, %, <<, >>, ^, AND, OR, XOR, <, >, <=, >=, ==
/// Result types: f/float (default), i/int, h/hex, c/char
pub struct FuncMath;

impl DialplanFunc for FuncMath {
    fn name(&self) -> &str {
        "MATH"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        // Split args into expression and optional type
        let parts: Vec<&str> = args.splitn(2, ',').collect();
        let expression = parts[0].trim();
        let result_type = parts
            .get(1)
            .map(|s| s.trim().to_lowercase())
            .unwrap_or_else(|| "f".to_string());

        // Parse and evaluate the expression
        let result = evaluate_expression(expression)?;

        // Format according to requested type
        match result_type.as_str() {
            "f" | "float" => {
                Ok(format!("{:.6}", result))
            }
            "i" | "int" => {
                Ok((result as i64).to_string())
            }
            "h" | "hex" => {
                Ok(format!("0x{:X}", result as i64))
            }
            "c" | "char" => {
                let ch = (result as u8) as char;
                Ok(ch.to_string())
            }
            _ => {
                Err(FuncError::InvalidArgument(format!(
                    "MATH: unknown result type '{}'",
                    result_type
                )))
            }
        }
    }
}

/// Evaluate a simple binary arithmetic expression.
///
/// Format: number1 operator number2
fn evaluate_expression(expr: &str) -> Result<f64, FuncError> {
    let expr = expr.trim();
    if expr.is_empty() {
        return Err(FuncError::InvalidArgument("MATH: empty expression".to_string()));
    }

    // Try to find the operator by checking for known operators
    // Order matters: check longer operators first
    let operators = [
        "<<", ">>", "<=", ">=", "==",
        "AND", "OR", "XOR",
        "+", "-", "*", "/", "%", "^", "<", ">",
    ];

    for op in &operators {
        // Find the operator, but not at the very beginning (to handle negative numbers)
        if let Some(op_pos) = find_operator(expr, op) {
            let left_str = expr[..op_pos].trim();
            let right_str = expr[op_pos + op.len()..].trim();

            let left = parse_number(left_str)?;
            let right = parse_number(right_str)?;

            return apply_operator(left, right, op);
        }
    }

    // If no operator found, try to parse as a single number
    parse_number(expr)
}

/// Find the position of an operator in the expression, skipping
/// leading negative signs.
fn find_operator(expr: &str, op: &str) -> Option<usize> {
    // For single-char ops like +, -, skip the first position
    // to handle negative numbers
    let start = if op.len() == 1 && (op == "+" || op == "-") {
        1
    } else {
        0
    };

    if start >= expr.len() {
        return None;
    }

    let search_area = &expr[start..];
    if op.chars().all(|c| c.is_alphabetic()) {
        // For word operators (AND, OR, XOR), require word boundaries
        if let Some(pos) = search_area.find(op) {
            let global_pos = start + pos;
            // Check boundaries
            let before_ok = global_pos == 0
                || !expr.as_bytes()[global_pos - 1].is_ascii_alphabetic();
            let after_ok = global_pos + op.len() >= expr.len()
                || !expr.as_bytes()[global_pos + op.len()].is_ascii_alphabetic();
            if before_ok && after_ok {
                return Some(global_pos);
            }
        }
        None
    } else {
        search_area.find(op).map(|pos| start + pos)
    }
}

/// Parse a number string (supports integer, float, hex).
fn parse_number(s: &str) -> Result<f64, FuncError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(FuncError::InvalidArgument(
            "MATH: empty number".to_string(),
        ));
    }

    // Hex
    if let Some(hex_digits) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        return i64::from_str_radix(hex_digits, 16)
            .map(|n| n as f64)
            .map_err(|_| {
                FuncError::InvalidArgument(format!("MATH: invalid hex number '{}'", s))
            });
    }

    // Float or integer
    s.parse::<f64>().map_err(|_| {
        FuncError::InvalidArgument(format!("MATH: invalid number '{}'", s))
    })
}

/// Apply a binary operator to two numbers.
fn apply_operator(left: f64, right: f64, op: &str) -> Result<f64, FuncError> {
    match op {
        "+" => Ok(left + right),
        "-" => Ok(left - right),
        "*" => Ok(left * right),
        "/" => {
            if right == 0.0 {
                Err(FuncError::InvalidArgument(
                    "MATH: division by zero".to_string(),
                ))
            } else {
                Ok(left / right)
            }
        }
        "%" => {
            if right == 0.0 {
                Err(FuncError::InvalidArgument(
                    "MATH: modulo by zero".to_string(),
                ))
            } else {
                Ok(left % right)
            }
        }
        "<<" => Ok(((left as i64) << (right as i64)) as f64),
        ">>" => Ok(((left as i64) >> (right as i64)) as f64),
        "^" => Ok(left.powf(right)),
        "AND" => Ok(((left as i64) & (right as i64)) as f64),
        "OR" => Ok(((left as i64) | (right as i64)) as f64),
        "XOR" => Ok(((left as i64) ^ (right as i64)) as f64),
        "<" => Ok(if left < right { 1.0 } else { 0.0 }),
        ">" => Ok(if left > right { 1.0 } else { 0.0 }),
        "<=" => Ok(if left <= right { 1.0 } else { 0.0 }),
        ">=" => Ok(if left >= right { 1.0 } else { 0.0 }),
        "==" => Ok(if (left - right).abs() < f64::EPSILON { 1.0 } else { 0.0 }),
        _ => Err(FuncError::InvalidArgument(format!(
            "MATH: unknown operator '{}'",
            op
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_addition() {
        let ctx = FuncContext::new();
        let func = FuncMath;
        let result = func.read(&ctx, "2+3,int").unwrap();
        assert_eq!(result, "5");
    }

    #[test]
    fn test_subtraction() {
        let ctx = FuncContext::new();
        let func = FuncMath;
        assert_eq!(func.read(&ctx, "10-3,int").unwrap(), "7");
    }

    #[test]
    fn test_multiplication() {
        let ctx = FuncContext::new();
        let func = FuncMath;
        assert_eq!(func.read(&ctx, "4*5,int").unwrap(), "20");
    }

    #[test]
    fn test_division() {
        let ctx = FuncContext::new();
        let func = FuncMath;
        assert_eq!(func.read(&ctx, "10/3,int").unwrap(), "3");
    }

    #[test]
    fn test_modulo() {
        let ctx = FuncContext::new();
        let func = FuncMath;
        assert_eq!(func.read(&ctx, "123%16,int").unwrap(), "11");
    }

    #[test]
    fn test_division_by_zero() {
        let ctx = FuncContext::new();
        let func = FuncMath;
        assert!(func.read(&ctx, "10/0").is_err());
    }

    #[test]
    fn test_hex_output() {
        let ctx = FuncContext::new();
        let func = FuncMath;
        assert_eq!(func.read(&ctx, "255+0,hex").unwrap(), "0xFF");
    }

    #[test]
    fn test_comparison() {
        let ctx = FuncContext::new();
        let func = FuncMath;
        assert_eq!(func.read(&ctx, "5>3,int").unwrap(), "1");
        assert_eq!(func.read(&ctx, "3>5,int").unwrap(), "0");
    }
}
