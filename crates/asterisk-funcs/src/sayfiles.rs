//! SAYFILES() function - return file lists for saying numbers/dates.
//!
//! Port of func_sayfiles.c from Asterisk C.
//!
//! Provides:
//! - SAYFILES(value,type) - return ampersand-delimited file list
//!
//! Types:
//! - digits: individual digit files
//! - number: number pronunciation files (English)
//! - alpha: letter-by-letter files
//! - phonetic: NATO phonetic alphabet files
//! - money: dollar/cent files (English, USD)
//! - ordinal: ordinal number files (English)

use crate::{DialplanFunc, FuncContext, FuncError, FuncResult};

/// SAYFILES() function.
///
/// Returns the ampersand-delimited file names that would be played
/// by the Say applications (SayDigits, SayNumber, etc.).
///
/// Usage: SAYFILES(value,type)
///
/// Supported types:
///   alpha    - SayAlpha files (letters/X for each character)
///   digits   - SayDigits files (digits/X for each digit)
///   number   - SayNumber files (English number pronunciation)
///   phonetic - SayPhonetic (NATO phonetic alphabet)
///   money    - SayMoney (USD, English)
///   ordinal  - SayOrdinal (English ordinal numbers)
pub struct FuncSayFiles;

impl FuncSayFiles {
    /// Generate digit file list: "digits/3&digits/5" for "35"
    fn say_digits(value: &str) -> String {
        let files: Vec<String> = value
            .chars()
            .filter(|c| c.is_ascii_digit())
            .map(|c| format!("digits/{}", c))
            .collect();
        files.join("&")
    }

    /// Generate alpha file list: "letters/h&letters/i" for "hi"
    fn say_alpha(value: &str) -> String {
        let mut files = Vec::new();
        for c in value.chars() {
            if c.is_ascii_alphabetic() {
                files.push(format!("letters/{}", c.to_ascii_lowercase()));
            } else if c.is_ascii_digit() {
                files.push(format!("digits/{}", c));
            } else if c == ' ' {
                files.push("letters/space".to_string());
            } else if c == '.' {
                files.push("letters/dot".to_string());
            } else if c == '@' {
                files.push("letters/at".to_string());
            }
        }
        files.join("&")
    }

    /// Generate phonetic file list using NATO alphabet.
    fn say_phonetic(value: &str) -> String {
        let files: Vec<String> = value
            .chars()
            .filter(|c| c.is_ascii_alphabetic())
            .map(|c| format!("phonetic/{}_p", c.to_ascii_lowercase()))
            .collect();
        files.join("&")
    }

    /// Generate number pronunciation files for English.
    /// Handles numbers up to billions.
    fn say_number(num: i64) -> String {
        if num == 0 {
            return "digits/0".to_string();
        }

        let mut files = Vec::new();
        let mut n = num.unsigned_abs();

        if num < 0 {
            files.push("letters/minus".to_string());
        }

        // Billions
        if n >= 1_000_000_000 {
            let billions = n / 1_000_000_000;
            files.extend(Self::say_number_under_thousand(billions));
            files.push("digits/billion".to_string());
            n %= 1_000_000_000;
        }

        // Millions
        if n >= 1_000_000 {
            let millions = n / 1_000_000;
            files.extend(Self::say_number_under_thousand(millions));
            files.push("digits/million".to_string());
            n %= 1_000_000;
        }

        // Thousands
        if n >= 1000 {
            let thousands = n / 1000;
            files.extend(Self::say_number_under_thousand(thousands));
            files.push("digits/thousand".to_string());
            n %= 1000;
        }

        // Hundreds
        if n >= 100 {
            let hundreds = n / 100;
            files.push(format!("digits/{}", hundreds));
            files.push("digits/hundred".to_string());
            n %= 100;
        }

        // Tens and ones
        if n > 0 {
            files.extend(Self::say_number_under_hundred(n));
        }

        files.join("&")
    }

    fn say_number_under_thousand(n: u64) -> Vec<String> {
        let mut files = Vec::new();
        let mut n = n;

        if n >= 100 {
            files.push(format!("digits/{}", n / 100));
            files.push("digits/hundred".to_string());
            n %= 100;
        }

        if n > 0 {
            files.extend(Self::say_number_under_hundred(n));
        }

        files
    }

    fn say_number_under_hundred(n: u64) -> Vec<String> {
        let mut files = Vec::new();
        if n == 0 {
            return files;
        }

        if n < 20 {
            files.push(format!("digits/{}", n));
        } else {
            let tens = (n / 10) * 10;
            files.push(format!("digits/{}", tens));
            let ones = n % 10;
            if ones > 0 {
                files.push(format!("digits/{}", ones));
            }
        }
        files
    }

    /// Generate ordinal number files for English.
    fn say_ordinal(num: i64) -> String {
        if num == 0 {
            return "digits/h-0".to_string();
        }

        let mut files = Vec::new();
        let mut n = num.unsigned_abs();

        // Billions
        if n >= 1_000_000_000 {
            let billions = n / 1_000_000_000;
            files.extend(Self::say_number_under_thousand(billions));
            n %= 1_000_000_000;
            if n == 0 {
                files.push("digits/h-billion".to_string());
                return files.join("&");
            }
            files.push("digits/billion".to_string());
        }

        // Millions
        if n >= 1_000_000 {
            let millions = n / 1_000_000;
            files.extend(Self::say_number_under_thousand(millions));
            n %= 1_000_000;
            if n == 0 {
                files.push("digits/h-million".to_string());
                return files.join("&");
            }
            files.push("digits/million".to_string());
        }

        // Thousands
        if n >= 1000 {
            let thousands = n / 1000;
            files.extend(Self::say_number_under_thousand(thousands));
            n %= 1000;
            if n == 0 {
                files.push("digits/h-thousand".to_string());
                return files.join("&");
            }
            files.push("digits/thousand".to_string());
        }

        // Hundreds
        if n >= 100 {
            let hundreds = n / 100;
            files.push(format!("digits/{}", hundreds));
            n %= 100;
            if n == 0 {
                files.push("digits/h-hundred".to_string());
                return files.join("&");
            }
            files.push("digits/hundred".to_string());
        }

        // Tens and ones - ordinal
        if n > 0 {
            if n < 20 {
                files.push(format!("digits/h-{}", n));
            } else {
                let tens = (n / 10) * 10;
                let ones = n % 10;
                if ones == 0 {
                    files.push(format!("digits/h-{}", tens));
                } else {
                    files.push(format!("digits/{}", tens));
                    files.push(format!("digits/h-{}", ones));
                }
            }
        }

        files.join("&")
    }

    /// Generate money pronunciation files for USD/English.
    fn say_money(value: &str) -> String {
        // Parse dollars and cents
        let (dollars, cents) = Self::parse_money(value);

        let mut files = Vec::new();

        if dollars == 0 && cents == 0 {
            files.push("digits/0".to_string());
            files.push("cents".to_string());
            return files.join("&");
        }

        if dollars > 0 {
            let dollar_files = Self::say_number(dollars as i64);
            files.push(dollar_files);

            if dollars == 1 {
                files.push("letters/dollar".to_string());
            } else {
                files.push("dollars".to_string());
            }

            if cents > 0 {
                files.push("and".to_string());
            }
        }

        if cents > 0 {
            let cent_files = Self::say_number(cents as i64);
            files.push(cent_files);

            if cents == 1 {
                files.push("cent".to_string());
            } else {
                files.push("cents".to_string());
            }
        }

        files.join("&")
    }

    /// Parse a money string into (dollars, cents).
    fn parse_money(value: &str) -> (u64, u64) {
        // Remove leading/trailing whitespace
        let value = value.trim();

        if let Some(dot_pos) = value.find('.') {
            let dollar_part = &value[..dot_pos];
            let cent_part = &value[dot_pos + 1..];

            // Parse dollars - stop at first non-digit
            let dollars: u64 = dollar_part
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .unwrap_or(0);

            // Parse cents - take first 2 digits only
            let cent_str: String = cent_part
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .take(2)
                .collect();

            let cents: u64 = if cent_str.is_empty() {
                0
            } else if cent_str.len() == 1 {
                cent_str.parse::<u64>().unwrap_or(0) * 10
            } else {
                cent_str.parse().unwrap_or(0)
            };

            (dollars, cents)
        } else {
            // No decimal - all dollars
            let dollars: u64 = value
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .unwrap_or(0);
            (dollars, 0)
        }
    }
}

impl DialplanFunc for FuncSayFiles {
    fn name(&self) -> &str {
        "SAYFILES"
    }

    fn read(&self, _ctx: &FuncContext, args: &str) -> FuncResult {
        let parts: Vec<&str> = args.splitn(2, ',').collect();
        if parts.is_empty() || parts[0].trim().is_empty() {
            return Err(FuncError::InvalidArgument(
                "SAYFILES: value argument is required".to_string(),
            ));
        }

        let value = parts[0].trim();
        let say_type = if parts.len() > 1 && !parts[1].trim().is_empty() {
            parts[1].trim()
        } else {
            "alpha"
        };

        let result = match say_type {
            "alpha" => Self::say_alpha(value),
            "digits" => Self::say_digits(value),
            "number" => {
                let num: i64 = value.parse().map_err(|_| {
                    FuncError::InvalidArgument(format!(
                        "SAYFILES: invalid numeric argument: {}",
                        value
                    ))
                })?;
                Self::say_number(num)
            }
            "ordinal" => {
                let num: i64 = value.parse().map_err(|_| {
                    FuncError::InvalidArgument(format!(
                        "SAYFILES: invalid numeric argument: {}",
                        value
                    ))
                })?;
                Self::say_ordinal(num)
            }
            "money" => Self::say_money(value),
            "phonetic" => Self::say_phonetic(value),
            other => {
                return Err(FuncError::InvalidArgument(format!(
                    "SAYFILES: invalid type '{}' (use alpha, digits, number, ordinal, money, phonetic)",
                    other
                )))
            }
        };

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_say_digits() {
        let ctx = FuncContext::new();
        let func = FuncSayFiles;
        assert_eq!(
            func.read(&ctx, "35,digits").unwrap(),
            "digits/3&digits/5"
        );
    }

    #[test]
    fn test_say_alpha() {
        let ctx = FuncContext::new();
        let func = FuncSayFiles;
        let result = func.read(&ctx, "hi,alpha").unwrap();
        assert_eq!(result, "letters/h&letters/i");
    }

    #[test]
    fn test_say_number_zero() {
        let ctx = FuncContext::new();
        let func = FuncSayFiles;
        assert_eq!(func.read(&ctx, "0,number").unwrap(), "digits/0");
    }

    #[test]
    fn test_say_number_35() {
        let ctx = FuncContext::new();
        let func = FuncSayFiles;
        assert_eq!(
            func.read(&ctx, "35,number").unwrap(),
            "digits/30&digits/5"
        );
    }

    #[test]
    fn test_say_number_747() {
        let ctx = FuncContext::new();
        let func = FuncSayFiles;
        assert_eq!(
            func.read(&ctx, "747,number").unwrap(),
            "digits/7&digits/hundred&digits/40&digits/7"
        );
    }

    #[test]
    fn test_say_number_1042() {
        let ctx = FuncContext::new();
        let func = FuncSayFiles;
        assert_eq!(
            func.read(&ctx, "1042,number").unwrap(),
            "digits/1&digits/thousand&digits/40&digits/2"
        );
    }

    #[test]
    fn test_say_phonetic() {
        let ctx = FuncContext::new();
        let func = FuncSayFiles;
        assert_eq!(
            func.read(&ctx, "abc,phonetic").unwrap(),
            "phonetic/a_p&phonetic/b_p&phonetic/c_p"
        );
    }

    #[test]
    fn test_say_ordinal_7() {
        let ctx = FuncContext::new();
        let func = FuncSayFiles;
        assert_eq!(func.read(&ctx, "7,ordinal").unwrap(), "digits/h-7");
    }

    #[test]
    fn test_say_ordinal_35() {
        let ctx = FuncContext::new();
        let func = FuncSayFiles;
        assert_eq!(
            func.read(&ctx, "35,ordinal").unwrap(),
            "digits/30&digits/h-5"
        );
    }

    #[test]
    fn test_say_money_zero() {
        let ctx = FuncContext::new();
        let func = FuncSayFiles;
        assert_eq!(func.read(&ctx, "0,money").unwrap(), "digits/0&cents");
    }

    #[test]
    fn test_say_money_one_dollar() {
        let ctx = FuncContext::new();
        let func = FuncSayFiles;
        let result = func.read(&ctx, "1.00,money").unwrap();
        assert!(result.contains("digits/1"));
        assert!(result.contains("dollar"));
    }

    #[test]
    fn test_say_money_with_cents() {
        let ctx = FuncContext::new();
        let func = FuncSayFiles;
        let result = func.read(&ctx, "2.42,money").unwrap();
        assert!(result.contains("digits/2"));
        assert!(result.contains("dollars"));
        assert!(result.contains("and"));
        assert!(result.contains("cents"));
    }

    #[test]
    fn test_say_money_only_cents() {
        let ctx = FuncContext::new();
        let func = FuncSayFiles;
        let result = func.read(&ctx, "0.01,money").unwrap();
        assert!(result.contains("digits/1"));
        assert!(result.contains("cent"));
    }

    #[test]
    fn test_missing_value() {
        let ctx = FuncContext::new();
        let func = FuncSayFiles;
        assert!(func.read(&ctx, "").is_err());
    }

    #[test]
    fn test_invalid_type() {
        let ctx = FuncContext::new();
        let func = FuncSayFiles;
        assert!(func.read(&ctx, "hello,bogus").is_err());
    }
}
