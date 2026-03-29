//! SPRINTF() function - C-style string formatting.
//!
//! Port of func_sprintf.c from Asterisk C.
//!
//! Usage: SPRINTF(format,arg1,arg2,...)
//!
//! Supported format specifiers:
//!   %s - string
//!   %d - integer
//!   %f - floating point
//!   %x - hexadecimal (lowercase)
//!   %X - hexadecimal (uppercase)
//!   %o - octal
//!   %c - character (from integer)
//!   %% - literal percent

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// SPRINTF() function.
///
/// Formats a string using C-style format specifiers.
///
/// Usage: SPRINTF(format,arg1,arg2,...)
pub struct FuncSprintf;

impl DialplanFunc for FuncSprintf {
    fn name(&self) -> &str {
        "SPRINTF"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        // Split into format string and arguments
        // The first argument is the format string, rest are substitution args
        let parts: Vec<&str> = args.splitn(2, ',').collect();
        let fmt_str = parts[0];
        let arg_str = parts.get(1).unwrap_or(&"");

        // Parse remaining arguments (comma-separated)
        let arguments: Vec<&str> = if arg_str.is_empty() {
            Vec::new()
        } else {
            arg_str.split(',').collect()
        };

        format_string(fmt_str, &arguments)
    }
}

/// Format a string using C-style format specifiers.
fn format_string(fmt: &str, args: &[&str]) -> FuncResult {
    let mut result = String::new();
    let chars: Vec<char> = fmt.chars().collect();
    let mut i = 0;
    let mut arg_idx = 0;

    while i < chars.len() {
        if chars[i] == '%' {
            i += 1;
            if i >= chars.len() {
                return Err(FuncError::InvalidArgument(
                    "SPRINTF: format string ends with bare %".to_string(),
                ));
            }

            // Handle %%
            if chars[i] == '%' {
                result.push('%');
                i += 1;
                continue;
            }

            // Parse optional flags
            let mut flags = FormatFlags::default();
            while i < chars.len() {
                match chars[i] {
                    '-' => { flags.left_align = true; i += 1; }
                    '+' => { flags.show_sign = true; i += 1; }
                    '0' if !flags.has_width => { flags.zero_pad = true; i += 1; }
                    ' ' => { flags.space_sign = true; i += 1; }
                    _ => break,
                }
            }

            // Parse optional width
            let width_start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            if i > width_start {
                flags.width = chars[width_start..i]
                    .iter()
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0);
                flags.has_width = true;
            }

            // Parse optional precision
            if i < chars.len() && chars[i] == '.' {
                i += 1;
                let prec_start = i;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
                flags.precision = if i > prec_start {
                    chars[prec_start..i]
                        .iter()
                        .collect::<String>()
                        .parse()
                        .unwrap_or(6)
                } else {
                    0
                };
                flags.has_precision = true;
            }

            // Parse conversion specifier
            if i >= chars.len() {
                return Err(FuncError::InvalidArgument(
                    "SPRINTF: incomplete format specifier".to_string(),
                ));
            }

            let specifier = chars[i];
            i += 1;

            let arg = if arg_idx < args.len() {
                args[arg_idx].trim()
            } else {
                ""
            };
            arg_idx += 1;

            let formatted = format_arg(specifier, arg, &flags)?;
            result.push_str(&formatted);
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    Ok(result)
}

#[derive(Default)]
struct FormatFlags {
    left_align: bool,
    zero_pad: bool,
    show_sign: bool,
    space_sign: bool,
    width: usize,
    has_width: bool,
    precision: usize,
    has_precision: bool,
}

/// Format a single argument according to the specifier.
fn format_arg(specifier: char, arg: &str, flags: &FormatFlags) -> Result<String, FuncError> {
    let raw = match specifier {
        's' => arg.to_string(),
        'd' | 'i' => {
            let n: i64 = arg.parse().unwrap_or(0);
            let mut s = if flags.show_sign && n >= 0 {
                format!("+{}", n)
            } else if flags.space_sign && n >= 0 {
                format!(" {}", n)
            } else {
                n.to_string()
            };
            if flags.zero_pad && flags.has_width && !flags.left_align {
                let sign = if s.starts_with('-') || s.starts_with('+') || s.starts_with(' ') {
                    let c = s.remove(0);
                    c.to_string()
                } else {
                    String::new()
                };
                while sign.len() + s.len() < flags.width {
                    s.insert(0, '0');
                }
                s = format!("{}{}", sign, s);
            }
            s
        }
        'f' => {
            let n: f64 = arg.parse().unwrap_or(0.0);
            let precision = if flags.has_precision {
                flags.precision
            } else {
                6
            };
            if flags.show_sign && n >= 0.0 {
                format!("+{:.*}", precision, n)
            } else {
                format!("{:.*}", precision, n)
            }
        }
        'x' => {
            let n: i64 = arg.parse().unwrap_or(0);
            format!("{:x}", n)
        }
        'X' => {
            let n: i64 = arg.parse().unwrap_or(0);
            format!("{:X}", n)
        }
        'o' => {
            let n: i64 = arg.parse().unwrap_or(0);
            format!("{:o}", n)
        }
        'c' => {
            let n: u32 = arg.parse().unwrap_or(0);
            char::from_u32(n).unwrap_or('?').to_string()
        }
        other => {
            return Err(FuncError::InvalidArgument(format!(
                "SPRINTF: unknown format specifier '%{}'",
                other
            )));
        }
    };

    // Apply width formatting
    if flags.has_width && raw.len() < flags.width {
        if flags.left_align {
            Ok(format!("{:<width$}", raw, width = flags.width))
        } else if !flags.zero_pad || specifier == 's' {
            Ok(format!("{:>width$}", raw, width = flags.width))
        } else {
            Ok(raw) // zero-pad already handled above for numerics
        }
    } else {
        Ok(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sprintf_string() {
        let ctx = FuncContext::new();
        let func = FuncSprintf;
        assert_eq!(func.read(&ctx, "Hello %s!,world").unwrap(), "Hello world!");
    }

    #[test]
    fn test_sprintf_integer() {
        let ctx = FuncContext::new();
        let func = FuncSprintf;
        assert_eq!(func.read(&ctx, "Count: %d,42").unwrap(), "Count: 42");
    }

    #[test]
    fn test_sprintf_float() {
        let ctx = FuncContext::new();
        let func = FuncSprintf;
        let result = func.read(&ctx, "Pi: %.2f,3.14159").unwrap();
        assert_eq!(result, "Pi: 3.14");
    }

    #[test]
    fn test_sprintf_hex() {
        let ctx = FuncContext::new();
        let func = FuncSprintf;
        assert_eq!(func.read(&ctx, "%x,255").unwrap(), "ff");
        assert_eq!(func.read(&ctx, "%X,255").unwrap(), "FF");
    }

    #[test]
    fn test_sprintf_multiple_args() {
        let ctx = FuncContext::new();
        let func = FuncSprintf;
        assert_eq!(
            func.read(&ctx, "%s=%d,key,42").unwrap(),
            "key=42"
        );
    }

    #[test]
    fn test_sprintf_percent_literal() {
        let ctx = FuncContext::new();
        let func = FuncSprintf;
        assert_eq!(func.read(&ctx, "100%%").unwrap(), "100%");
    }

    #[test]
    fn test_sprintf_width() {
        let ctx = FuncContext::new();
        let func = FuncSprintf;
        assert_eq!(func.read(&ctx, "%5d,42").unwrap(), "   42");
        assert_eq!(func.read(&ctx, "%-5d!,42").unwrap(), "42   !");
    }

    #[test]
    fn test_sprintf_zero_pad() {
        let ctx = FuncContext::new();
        let func = FuncSprintf;
        assert_eq!(func.read(&ctx, "%05d,42").unwrap(), "00042");
    }
}
