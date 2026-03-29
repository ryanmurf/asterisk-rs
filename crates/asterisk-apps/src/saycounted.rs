//! SayCountedNoun and SayCountedAdj applications.
//!
//! Port of app_saycounted.c from Asterisk C. Provides language-aware
//! declension of nouns and adjectives for counted items. For English,
//! selects singular/plural forms. For Slavic languages, selects
//! nominative/genitive singular/genitive plural forms.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::{info, warn};

/// Gender for adjective declension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gender {
    Masculine,
    Feminine,
    Neuter,
    Common,
}

impl Gender {
    /// Parse a gender character.
    pub fn from_char(ch: char) -> Option<Self> {
        match ch {
            'm' | 'M' => Some(Self::Masculine),
            'f' | 'F' => Some(Self::Feminine),
            'n' | 'N' => Some(Self::Neuter),
            'c' | 'C' => Some(Self::Common),
            _ => None,
        }
    }

    /// Get the suffix character for this gender.
    pub fn suffix(&self) -> &str {
        match self {
            Self::Masculine => "m",
            Self::Feminine => "f",
            Self::Neuter => "n",
            Self::Common => "c",
        }
    }
}

/// Determine the appropriate noun suffix based on count and language.
///
/// For English:
///   - count == 1: empty suffix (singular)
///   - count != 1: "s" suffix (plural)
///
/// For Slavic languages (Russian, etc.):
///   - count == 1: empty suffix (nominative singular)
///   - count 2-4 (last two digits): "x1" suffix (genitive singular)
///   - everything else: "x2" suffix (genitive plural)
pub fn noun_suffix(count: i32, language: &str) -> &'static str {
    if language.starts_with("ru") || language.starts_with("ua") || language.starts_with("pl") {
        // Slavic language rules
        let abs = count.unsigned_abs();
        let last_two = abs % 100;
        let last_one = abs % 10;

        if last_two >= 11 && last_two <= 19 {
            "x2" // genitive plural for teens
        } else if last_one == 1 {
            "" // nominative singular
        } else if last_one >= 2 && last_one <= 4 {
            "x1" // genitive singular
        } else {
            "x2" // genitive plural
        }
    } else {
        // English and other languages with simple singular/plural
        if count == 1 || count == -1 {
            ""
        } else {
            "s"
        }
    }
}

/// Determine the appropriate adjective suffix based on count, gender, and language.
///
/// For English: always empty (adjectives are not declined).
/// For Slavic languages:
///   - count == 1: gender suffix
///   - otherwise: "x" (genitive plural form)
pub fn adjective_suffix(count: i32, gender: Option<Gender>, language: &str) -> &'static str {
    if language.starts_with("ru") || language.starts_with("ua") || language.starts_with("pl") {
        let abs = count.unsigned_abs();
        let last_two = abs % 100;
        let last_one = abs % 10;

        if last_one == 1 && last_two != 11 {
            // Nominative singular: use gender suffix
            match gender {
                Some(Gender::Masculine) => "m",
                Some(Gender::Feminine) => "f",
                Some(Gender::Neuter) => "n",
                Some(Gender::Common) => "c",
                None => "",
            }
        } else {
            "x" // genitive plural
        }
    } else {
        "" // English: no declension
    }
}

/// The SayCountedNoun() dialplan application.
///
/// Usage: SayCountedNoun(number,filename)
///
/// Plays the correct singular or plural form of a noun. The suffix is
/// appended to the filename: for English, "s" is appended for plural
/// (e.g., "call" vs "calls").
///
/// Does not automatically answer the channel.
pub struct AppSayCountedNoun;

impl DialplanApp for AppSayCountedNoun {
    fn name(&self) -> &str {
        "SayCountedNoun"
    }

    fn description(&self) -> &str {
        "Say a noun in declined form to count things"
    }
}

impl AppSayCountedNoun {
    /// Execute the SayCountedNoun application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parts: Vec<&str> = args.splitn(2, ',').collect();
        if parts.len() < 2 {
            warn!("SayCountedNoun: requires two arguments (number,noun)");
            return PbxExecResult::Failed;
        }

        let number: i32 = match parts[0].trim().parse() {
            Ok(n) => n,
            Err(_) => {
                warn!("SayCountedNoun: first argument must be a number");
                return PbxExecResult::Failed;
            }
        };

        let noun = parts[1].trim();
        if noun.is_empty() {
            warn!("SayCountedNoun: noun filename required");
            return PbxExecResult::Failed;
        }

        // Determine language from channel (default to English)
        let language = "en";

        let suffix = noun_suffix(number, language);
        let filename = format!("{}{}", noun, suffix);

        info!(
            "SayCountedNoun: channel '{}' saying '{}' (count={}, suffix='{}')",
            channel.name, filename, number, suffix,
        );

        // In a real implementation:
        //   play_file(channel, &filename).await;

        PbxExecResult::Success
    }
}

/// The SayCountedAdj() dialplan application.
///
/// Usage: SayCountedAdj(number,filename[,gender])
///
/// Plays the correct form of an adjective based on count and gender.
/// For English, the adjective is not declined. For Slavic languages,
/// the suffix depends on gender and count.
///
/// Gender: m (masculine), f (feminine), n (neuter), c (common).
/// Does not automatically answer the channel.
pub struct AppSayCountedAdj;

impl DialplanApp for AppSayCountedAdj {
    fn name(&self) -> &str {
        "SayCountedAdj"
    }

    fn description(&self) -> &str {
        "Say an adjective in declined form to count things"
    }
}

impl AppSayCountedAdj {
    /// Execute the SayCountedAdj application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parts: Vec<&str> = args.splitn(3, ',').collect();
        if parts.len() < 2 {
            warn!("SayCountedAdj: requires at least two arguments (number,adjective[,gender])");
            return PbxExecResult::Failed;
        }

        let number: i32 = match parts[0].trim().parse() {
            Ok(n) => n,
            Err(_) => {
                warn!("SayCountedAdj: first argument must be a number");
                return PbxExecResult::Failed;
            }
        };

        let adjective = parts[1].trim();
        if adjective.is_empty() {
            warn!("SayCountedAdj: adjective filename required");
            return PbxExecResult::Failed;
        }

        let gender = parts
            .get(2)
            .and_then(|g| g.trim().chars().next())
            .and_then(Gender::from_char);

        let language = "en";
        let suffix = adjective_suffix(number, gender, language);
        let filename = format!("{}{}", adjective, suffix);

        info!(
            "SayCountedAdj: channel '{}' saying '{}' (count={}, gender={:?})",
            channel.name, filename, number, gender,
        );

        // In a real implementation:
        //   play_file(channel, &filename).await;

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_noun_suffix_english_singular() {
        assert_eq!(noun_suffix(1, "en"), "");
    }

    #[test]
    fn test_noun_suffix_english_plural() {
        assert_eq!(noun_suffix(0, "en"), "s");
        assert_eq!(noun_suffix(2, "en"), "s");
        assert_eq!(noun_suffix(5, "en"), "s");
        assert_eq!(noun_suffix(100, "en"), "s");
    }

    #[test]
    fn test_noun_suffix_russian() {
        assert_eq!(noun_suffix(1, "ru"), "");      // nominative
        assert_eq!(noun_suffix(2, "ru"), "x1");    // genitive singular
        assert_eq!(noun_suffix(3, "ru"), "x1");
        assert_eq!(noun_suffix(4, "ru"), "x1");
        assert_eq!(noun_suffix(5, "ru"), "x2");    // genitive plural
        assert_eq!(noun_suffix(11, "ru"), "x2");   // teen exception
        assert_eq!(noun_suffix(12, "ru"), "x2");
        assert_eq!(noun_suffix(21, "ru"), "");      // 21 -> nominative
        assert_eq!(noun_suffix(22, "ru"), "x1");
    }

    #[test]
    fn test_adjective_suffix_english() {
        assert_eq!(adjective_suffix(1, Some(Gender::Masculine), "en"), "");
        assert_eq!(adjective_suffix(5, Some(Gender::Feminine), "en"), "");
    }

    #[test]
    fn test_adjective_suffix_russian() {
        assert_eq!(adjective_suffix(1, Some(Gender::Feminine), "ru"), "f");
        assert_eq!(adjective_suffix(5, Some(Gender::Feminine), "ru"), "x");
        assert_eq!(adjective_suffix(21, Some(Gender::Masculine), "ru"), "m");
    }

    #[test]
    fn test_gender_from_char() {
        assert_eq!(Gender::from_char('m'), Some(Gender::Masculine));
        assert_eq!(Gender::from_char('f'), Some(Gender::Feminine));
        assert_eq!(Gender::from_char('n'), Some(Gender::Neuter));
        assert_eq!(Gender::from_char('c'), Some(Gender::Common));
        assert_eq!(Gender::from_char('x'), None);
    }

    #[tokio::test]
    async fn test_saycountednoun_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppSayCountedNoun::exec(&mut channel, "5,call").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_saycountedadj_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppSayCountedAdj::exec(&mut channel, "1,new,f").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_saycountednoun_bad_args() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppSayCountedNoun::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Failed);
    }
}
