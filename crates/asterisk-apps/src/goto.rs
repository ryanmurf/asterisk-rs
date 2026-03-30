//! Goto dialplan application.
//!
//! Port of core goto functionality from Asterisk C. Provides Goto()
//! for unconditional jumps in the dialplan.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::{info, warn};

/// A parsed dialplan goto target.
#[derive(Debug, Clone)]
pub struct GotoTarget {
    /// Target context (None = stay in current).
    pub context: Option<String>,
    /// Target extension (None = stay in current).
    pub extension: Option<String>,
    /// Target priority.
    pub priority: String,
}

impl GotoTarget {
    /// Parse from comma-separated arguments.
    ///
    /// Formats:
    ///   priority
    ///   extension,priority
    ///   context,extension,priority
    pub fn parse(args: &str) -> Option<Self> {
        let parts: Vec<&str> = args.split(',').map(|s| s.trim()).collect();
        match parts.len() {
            1 => Some(Self {
                context: None,
                extension: None,
                priority: parts[0].to_string(),
            }),
            2 => Some(Self {
                context: None,
                extension: Some(parts[0].to_string()),
                priority: parts[1].to_string(),
            }),
            3 => Some(Self {
                context: Some(parts[0].to_string()),
                extension: Some(parts[1].to_string()),
                priority: parts[2].to_string(),
            }),
            _ => None,
        }
    }
}

/// The Goto() dialplan application.
///
/// Usage: Goto([[context,]extension,]priority)
///
/// Unconditional jump to a dialplan location. The priority can be
/// a number or a label. If context and/or extension are omitted,
/// the current values are used.
pub struct AppGoto;

impl DialplanApp for AppGoto {
    fn name(&self) -> &str {
        "Goto"
    }

    fn description(&self) -> &str {
        "Unconditional goto in the dialplan"
    }
}

impl AppGoto {
    /// Execute the Goto application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let target = match GotoTarget::parse(args) {
            Some(t) => t,
            None => {
                warn!("Goto: requires [[context,]extension,]priority argument");
                return PbxExecResult::Failed;
            }
        };

        info!(
            "Goto: channel '{}' -> {},{},{}",
            channel.name,
            target.context.as_deref().unwrap_or("(current)"),
            target.extension.as_deref().unwrap_or("(current)"),
            target.priority,
        );

        // Set the channel's dialplan location so pbx_run continues
        // from the new position.
        if let Some(ctx) = &target.context {
            channel.context = ctx.clone();
        }
        if let Some(ext) = &target.extension {
            channel.exten = ext.clone();
        }

        // Parse priority: can be a number or a label like "n" (next)
        let priority: i32 = match target.priority.parse::<i32>() {
            Ok(p) => p,
            Err(_) => {
                // Label-based priority -- "n" means next, for now treat
                // unknown labels as priority 1
                if target.priority.eq_ignore_ascii_case("n") {
                    channel.priority + 1
                } else {
                    warn!("Goto: unknown priority label '{}', using 1", target.priority);
                    1
                }
            }
        };
        channel.priority = priority;

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_goto_target_priority_only() {
        let t = GotoTarget::parse("100").unwrap();
        assert!(t.context.is_none());
        assert!(t.extension.is_none());
        assert_eq!(t.priority, "100");
    }

    #[test]
    fn test_goto_target_exten_priority() {
        let t = GotoTarget::parse("s,1").unwrap();
        assert!(t.context.is_none());
        assert_eq!(t.extension.as_deref(), Some("s"));
        assert_eq!(t.priority, "1");
    }

    #[test]
    fn test_goto_target_full() {
        let t = GotoTarget::parse("default,s,1").unwrap();
        assert_eq!(t.context.as_deref(), Some("default"));
        assert_eq!(t.extension.as_deref(), Some("s"));
        assert_eq!(t.priority, "1");
    }

    #[tokio::test]
    async fn test_goto_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppGoto::exec(&mut channel, "default,s,1").await;
        assert_eq!(result, PbxExecResult::Success);
    }
}
