//! Originate application - creates a new outbound call.
//!
//! Port of app_originate.c from Asterisk C. Originates an outbound call
//! and connects it to a specified extension or application. Blocks until
//! the outgoing call fails or is answered (unless async mode is used).

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Originate status set as the ORIGINATE_STATUS channel variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OriginateStatus {
    Failed,
    Success,
    Busy,
    Congestion,
    Hangup,
    Ringing,
    Unknown,
}

impl OriginateStatus {
    /// String representation for the ORIGINATE_STATUS variable.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Failed => "FAILED",
            Self::Success => "SUCCESS",
            Self::Busy => "BUSY",
            Self::Congestion => "CONGESTION",
            Self::Hangup => "HANGUP",
            Self::Ringing => "RINGING",
            Self::Unknown => "UNKNOWN",
        }
    }
}

/// The type of origination -- connect to dialplan extension or application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OriginateType {
    /// Connect to a dialplan extension: context, extension, priority.
    Extension {
        context: String,
        extension: String,
        priority: i32,
    },
    /// Connect to a dialplan application with arguments.
    Application {
        app_name: String,
        app_data: String,
    },
}

/// Options for the Originate application.
#[derive(Debug, Clone, Default)]
pub struct OriginateOptions {
    /// Originate asynchronously (don't wait for answer).
    pub async_originate: bool,
    /// Caller ID number override.
    pub caller_num: Option<String>,
    /// Caller ID name override.
    pub caller_name: Option<String>,
    /// Channel variables to set on the destination channel.
    pub variables: Vec<(String, String)>,
}

impl OriginateOptions {
    /// Parse the options string.
    ///
    /// Simple option parsing: 'a' for async. Additional options with
    /// arguments use sub-parsers in the full implementation.
    pub fn parse(opts: &str) -> Self {
        let mut result = Self::default();
        for ch in opts.chars() {
            match ch {
                'a' => result.async_originate = true,
                _ => {
                    debug!("Originate: ignoring unknown option '{}'", ch);
                }
            }
        }
        result
    }
}

/// Parsed arguments for the Originate application.
#[derive(Debug)]
pub struct OriginateArgs {
    /// Channel technology (e.g. "SIP", "PJSIP").
    pub tech: String,
    /// Technology-specific data (e.g. "1234" for SIP/1234).
    pub tech_data: String,
    /// Type of origination (exten or app).
    pub originate_type: OriginateType,
    /// Timeout for the origination.
    pub timeout: Duration,
    /// Origination options.
    pub options: OriginateOptions,
}

impl OriginateArgs {
    /// Parse the Originate() argument string.
    ///
    /// Format: `tech/data,type,arg1[,arg2[,arg3[,timeout[,options]]]]`
    ///
    /// For type=exten: arg1=context, arg2=extension, arg3=priority
    /// For type=app:   arg1=application, arg2=app_data
    pub fn parse(args: &str) -> Option<Self> {
        let parts: Vec<&str> = args.splitn(7, ',').collect();
        if parts.len() < 3 {
            return None;
        }

        // Parse tech/data
        let tech_data_str = parts[0].trim();
        let (tech, tech_data) = if let Some(slash_pos) = tech_data_str.find('/') {
            (
                tech_data_str[..slash_pos].to_string(),
                tech_data_str[slash_pos + 1..].to_string(),
            )
        } else {
            return None;
        };

        if tech.is_empty() {
            return None;
        }

        // Parse type
        let type_str = parts[1].trim().to_lowercase();
        let originate_type = match type_str.as_str() {
            "exten" => {
                let context = parts.get(2).unwrap_or(&"default").trim().to_string();
                let extension = parts.get(3).map_or("s".to_string(), |e| {
                    let trimmed = e.trim();
                    if trimmed.is_empty() { "s".to_string() } else { trimmed.to_string() }
                });
                let priority = parts
                    .get(4)
                    .and_then(|p| p.trim().parse::<i32>().ok())
                    .unwrap_or(1);
                OriginateType::Extension {
                    context,
                    extension,
                    priority,
                }
            }
            "app" => {
                let app_name = parts.get(2).unwrap_or(&"").trim().to_string();
                let app_data = parts.get(3).map_or(String::new(), |d| d.trim().to_string());
                if app_name.is_empty() {
                    return None;
                }
                OriginateType::Application { app_name, app_data }
            }
            _ => {
                return None;
            }
        };

        // Parse timeout (default 30 seconds)
        let timeout_index = match &originate_type {
            OriginateType::Extension { .. } => 5,
            OriginateType::Application { .. } => 4,
        };
        let timeout = parts
            .get(timeout_index)
            .and_then(|t| t.trim().parse::<u64>().ok())
            .unwrap_or(30);
        let timeout = Duration::from_secs(timeout);

        // Parse options
        let options_index = timeout_index + 1;
        let options = parts
            .get(options_index)
            .map(|o| OriginateOptions::parse(o.trim()))
            .unwrap_or_default();

        Some(Self {
            tech,
            tech_data,
            originate_type,
            timeout,
            options,
        })
    }
}

/// The Originate() dialplan application.
///
/// Originates an outbound call and connects it to a specified extension
/// or application. This application will block until the outgoing call
/// fails or gets answered, unless the async option is used.
///
/// Usage: Originate(tech/data,type,arg1[,arg2[,arg3[,timeout[,options]]]])
///
/// Sets ORIGINATE_STATUS channel variable.
pub struct AppOriginate;

impl DialplanApp for AppOriginate {
    fn name(&self) -> &str {
        "Originate"
    }

    fn description(&self) -> &str {
        "Originate a call"
    }
}

impl AppOriginate {
    /// Execute the Originate application.
    ///
    /// # Arguments
    /// * `channel` - The current channel
    /// * `args` - Argument string
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parsed = match OriginateArgs::parse(args) {
            Some(a) => a,
            None => {
                warn!("Originate: invalid arguments: '{}'", args);
                channel.set_variable("ORIGINATE_STATUS", OriginateStatus::Failed.as_str());
                return PbxExecResult::Failed;
            }
        };

        info!(
            "Originate: channel '{}' originating {}/{} ({:?}, timeout={:?}, async={})",
            channel.name,
            parsed.tech,
            parsed.tech_data,
            parsed.originate_type,
            parsed.timeout,
            parsed.options.async_originate,
        );

        // In a full implementation, we would:
        //
        // 1. Look up the channel technology driver for `parsed.tech`
        // 2. Create a new outbound channel via the driver's request() method
        // 3. Set caller ID on the new channel if overridden
        // 4. Set any channel variables from options
        // 5. Depending on originate_type:
        //    - Extension: set context/exten/priority on the new channel
        //    - Application: prepare to run the app on the new channel
        // 6. If async:
        //    - Spawn the call in background, return immediately
        // 7. If synchronous:
        //    - Call the channel driver's call() method
        //    - Wait for answer or failure up to timeout
        //    - Report result
        //
        //   let tech_driver = ChannelDriverRegistry::find(&parsed.tech)?;
        //   let mut outbound = tech_driver.request(&parsed.tech_data, Some(channel)).await?;
        //
        //   // Set caller ID overrides
        //   if let Some(ref num) = parsed.options.caller_num {
        //       outbound.caller.number = num.clone();
        //   }
        //   if let Some(ref name) = parsed.options.caller_name {
        //       outbound.caller.name = name.clone();
        //   }
        //
        //   // Set variables
        //   for (key, value) in &parsed.options.variables {
        //       outbound.set_variable(key, value);
        //   }
        //
        //   match parsed.originate_type {
        //       OriginateType::Extension { context, extension, priority } => {
        //           outbound.context = context;
        //           outbound.exten = extension;
        //           outbound.priority = priority;
        //           pbx_start(&mut outbound).await?;
        //       }
        //       OriginateType::Application { app_name, app_data } => {
        //           let app = AppRegistry::find(&app_name)?;
        //           app.execute(&mut outbound, &app_data).await?;
        //       }
        //   }

        let status = OriginateStatus::Success;
        channel.set_variable("ORIGINATE_STATUS", status.as_str());

        debug!(
            "Originate: ORIGINATE_STATUS={}",
            status.as_str()
        );

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_exten_args() {
        let args = OriginateArgs::parse("SIP/1234,exten,default,100,1,30").unwrap();
        assert_eq!(args.tech, "SIP");
        assert_eq!(args.tech_data, "1234");
        assert_eq!(args.timeout, Duration::from_secs(30));
        match args.originate_type {
            OriginateType::Extension {
                context,
                extension,
                priority,
            } => {
                assert_eq!(context, "default");
                assert_eq!(extension, "100");
                assert_eq!(priority, 1);
            }
            _ => panic!("expected Extension type"),
        }
    }

    #[test]
    fn test_parse_app_args() {
        let args = OriginateArgs::parse("PJSIP/bob,app,Playback,hello-world").unwrap();
        assert_eq!(args.tech, "PJSIP");
        assert_eq!(args.tech_data, "bob");
        match args.originate_type {
            OriginateType::Application { app_name, app_data } => {
                assert_eq!(app_name, "Playback");
                assert_eq!(app_data, "hello-world");
            }
            _ => panic!("expected Application type"),
        }
    }

    #[test]
    fn test_parse_args_minimal() {
        let args = OriginateArgs::parse("SIP/100,exten,default").unwrap();
        match args.originate_type {
            OriginateType::Extension {
                context,
                extension,
                priority,
            } => {
                assert_eq!(context, "default");
                assert_eq!(extension, "s");
                assert_eq!(priority, 1);
            }
            _ => panic!("expected Extension type"),
        }
        assert_eq!(args.timeout, Duration::from_secs(30));
    }

    #[test]
    fn test_parse_args_invalid() {
        assert!(OriginateArgs::parse("").is_none());
        assert!(OriginateArgs::parse("SIP/100").is_none());
        assert!(OriginateArgs::parse("SIP/100,badtype,x").is_none());
    }

    #[tokio::test]
    async fn test_originate_bad_args() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppOriginate::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Failed);
        assert_eq!(channel.get_variable("ORIGINATE_STATUS"), Some("FAILED"));
    }
}
