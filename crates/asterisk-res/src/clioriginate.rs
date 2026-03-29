//! CLI originate command.
//!
//! Port of `res/res_clioriginate.c`. Implements the CLI "channel originate"
//! command, which creates an outbound call from the command line and
//! connects it to either a dialplan extension or a specific application.

use std::fmt;

use thiserror::Error;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum CliOriginateError {
    #[error("invalid channel specification: {0}")]
    InvalidChannel(String),
    #[error("invalid extension: {0}")]
    InvalidExtension(String),
    #[error("originate failed: {0}")]
    OriginateFailed(String),
    #[error("parse error: {0}")]
    ParseError(String),
}

pub type CliOriginateResult<T> = Result<T, CliOriginateError>;

// ---------------------------------------------------------------------------
// Originate target
// ---------------------------------------------------------------------------

/// Target for an originated call.
#[derive(Debug, Clone)]
pub enum OriginateTarget {
    /// Connect to a dialplan extension.
    Extension {
        context: String,
        extension: String,
        priority: i32,
    },
    /// Connect to an application directly.
    Application { app: String, app_data: String },
}

impl fmt::Display for OriginateTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Extension {
                context,
                extension,
                priority,
            } => write!(f, "{}@{},{}", extension, context, priority),
            Self::Application { app, app_data } => {
                if app_data.is_empty() {
                    write!(f, "{}", app)
                } else {
                    write!(f, "{}({})", app, app_data)
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Originate request
// ---------------------------------------------------------------------------

/// A parsed CLI originate request.
#[derive(Debug, Clone)]
pub struct OriginateRequest {
    /// Channel technology and address (e.g., "PJSIP/alice").
    pub channel: String,
    /// Where to connect the call.
    pub target: OriginateTarget,
    /// Caller ID string (optional).
    pub caller_id: Option<String>,
    /// Timeout in seconds for the outbound leg.
    pub timeout: u32,
}

impl OriginateRequest {
    /// Create an originate request to an extension.
    pub fn to_extension(
        channel: &str,
        extension: &str,
        context: &str,
        priority: i32,
    ) -> Self {
        Self {
            channel: channel.to_string(),
            target: OriginateTarget::Extension {
                context: context.to_string(),
                extension: extension.to_string(),
                priority,
            },
            caller_id: None,
            timeout: 30,
        }
    }

    /// Create an originate request to an application.
    pub fn to_application(channel: &str, app: &str, app_data: &str) -> Self {
        Self {
            channel: channel.to_string(),
            target: OriginateTarget::Application {
                app: app.to_string(),
                app_data: app_data.to_string(),
            },
            caller_id: None,
            timeout: 30,
        }
    }

    pub fn with_caller_id(mut self, cid: &str) -> Self {
        self.caller_id = Some(cid.to_string());
        self
    }

    pub fn with_timeout(mut self, timeout: u32) -> Self {
        self.timeout = timeout;
        self
    }

    /// Validate the request.
    pub fn validate(&self) -> CliOriginateResult<()> {
        if !self.channel.contains('/') {
            return Err(CliOriginateError::InvalidChannel(format!(
                "Channel must be in tech/address format: {}",
                self.channel
            )));
        }
        match &self.target {
            OriginateTarget::Extension { extension, .. } if extension.is_empty() => {
                return Err(CliOriginateError::InvalidExtension(
                    "Extension cannot be empty".into(),
                ));
            }
            OriginateTarget::Application { app, .. } if app.is_empty() => {
                return Err(CliOriginateError::ParseError(
                    "Application name cannot be empty".into(),
                ));
            }
            _ => {}
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// CLI command parser
// ---------------------------------------------------------------------------

/// Parse a "channel originate" CLI command line.
///
/// Supports:
/// - `channel originate <tech/data> extension <exten@context>`
/// - `channel originate <tech/data> extension <exten@context> <priority>`
/// - `channel originate <tech/data> application <app> [data]`
pub fn parse_originate_command(args: &[&str]) -> CliOriginateResult<OriginateRequest> {
    // Expect at minimum: ["channel", "originate", "<channel>", "<type>", "<value>"]
    // or just the args after "channel originate": ["<channel>", "<type>", "<value>"]
    let args: Vec<&str> = if args.first() == Some(&"channel") {
        if args.get(1) == Some(&"originate") {
            args[2..].to_vec()
        } else {
            args[1..].to_vec()
        }
    } else {
        args.to_vec()
    };

    if args.len() < 3 {
        return Err(CliOriginateError::ParseError(
            "Usage: channel originate <tech/data> extension <exten@context> | application <app> [data]".into(),
        ));
    }

    let channel = args[0];
    let target_type = args[1].to_lowercase();

    match target_type.as_str() {
        "extension" => {
            let exten_str = args[2];
            let (extension, context) = if let Some(at_pos) = exten_str.find('@') {
                (&exten_str[..at_pos], &exten_str[at_pos + 1..])
            } else {
                (exten_str, "default")
            };

            let priority = if args.len() > 3 {
                args[3]
                    .parse()
                    .unwrap_or(1)
            } else {
                1
            };

            Ok(OriginateRequest::to_extension(
                channel, extension, context, priority,
            ))
        }
        "application" => {
            let app = args[2];
            let app_data = if args.len() > 3 {
                args[3..].join(" ")
            } else {
                String::new()
            };
            Ok(OriginateRequest::to_application(channel, app, &app_data))
        }
        _ => Err(CliOriginateError::ParseError(format!(
            "Unknown target type '{}'. Use 'extension' or 'application'.",
            target_type
        ))),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_extension() {
        let req = parse_originate_command(&[
            "channel",
            "originate",
            "PJSIP/alice",
            "extension",
            "100@default",
        ])
        .unwrap();

        assert_eq!(req.channel, "PJSIP/alice");
        match &req.target {
            OriginateTarget::Extension {
                context,
                extension,
                priority,
            } => {
                assert_eq!(extension, "100");
                assert_eq!(context, "default");
                assert_eq!(*priority, 1);
            }
            _ => panic!("Expected Extension target"),
        }
    }

    #[test]
    fn test_parse_extension_with_priority() {
        let req = parse_originate_command(&[
            "PJSIP/bob",
            "extension",
            "200@internal",
            "5",
        ])
        .unwrap();

        match &req.target {
            OriginateTarget::Extension {
                context,
                extension,
                priority,
            } => {
                assert_eq!(extension, "200");
                assert_eq!(context, "internal");
                assert_eq!(*priority, 5);
            }
            _ => panic!("Expected Extension target"),
        }
    }

    #[test]
    fn test_parse_application() {
        let req = parse_originate_command(&[
            "PJSIP/alice",
            "application",
            "Playback",
            "hello-world",
        ])
        .unwrap();

        assert_eq!(req.channel, "PJSIP/alice");
        match &req.target {
            OriginateTarget::Application { app, app_data } => {
                assert_eq!(app, "Playback");
                assert_eq!(app_data, "hello-world");
            }
            _ => panic!("Expected Application target"),
        }
    }

    #[test]
    fn test_parse_no_context() {
        let req =
            parse_originate_command(&["SIP/bob", "extension", "100"]).unwrap();
        match &req.target {
            OriginateTarget::Extension { context, .. } => {
                assert_eq!(context, "default");
            }
            _ => panic!("Expected Extension"),
        }
    }

    #[test]
    fn test_parse_too_few_args() {
        let result = parse_originate_command(&["PJSIP/alice"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_bad_channel() {
        let req = OriginateRequest::to_extension("nochannel", "100", "default", 1);
        assert!(req.validate().is_err());
    }

    #[test]
    fn test_validate_good_request() {
        let req = OriginateRequest::to_extension("PJSIP/alice", "100", "default", 1);
        assert!(req.validate().is_ok());
    }

    #[test]
    fn test_target_display() {
        let ext = OriginateTarget::Extension {
            context: "default".into(),
            extension: "100".into(),
            priority: 1,
        };
        assert_eq!(format!("{}", ext), "100@default,1");

        let app = OriginateTarget::Application {
            app: "Playback".into(),
            app_data: "hello".into(),
        };
        assert_eq!(format!("{}", app), "Playback(hello)");
    }
}
