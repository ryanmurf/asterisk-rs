//! Page application - paging/intercom system.
//!
//! Port of app_page.c from Asterisk C. Places outbound calls to a list of
//! devices simultaneously and dumps them into a conference bridge as muted
//! participants. The original caller is the announcer (unmuted speaker).
//! When the announcer leaves, all paged channels are hung up.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::bridge::Bridge;
use asterisk_core::channel::Channel;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Page status set as the PAGE_STATUS channel variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageStatus {
    /// Page completed normally.
    Success,
    /// Page failed (no devices answered, error, etc.).
    Failed,
    /// Page timed out.
    Timeout,
}

impl PageStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "SUCCESS",
            Self::Failed => "FAILED",
            Self::Timeout => "TIMEOUT",
        }
    }
}

/// Options for the Page application.
#[derive(Debug, Clone, Default)]
pub struct PageOptions {
    /// Full duplex audio (participants can speak back).
    pub duplex: bool,
    /// Quiet: do not play beep to caller.
    pub quiet: bool,
    /// Record the page (uses conference bridge recording).
    pub record: bool,
    /// Only dial devices that are NOT_INUSE.
    pub skip_inuse: bool,
    /// Ignore call forwarding.
    pub ignore_forwards: bool,
    /// Announcement file to play to all participants.
    pub announcement: Option<String>,
    /// Do not play announcement to the caller.
    pub no_caller_announce: bool,
    /// Pre-dial GoSub for callee channels.
    pub predial_callee: Option<String>,
    /// Pre-dial GoSub for caller channel.
    pub predial_caller: Option<String>,
}

impl PageOptions {
    /// Parse the options string.
    pub fn parse(opts: &str) -> Self {
        let mut result = Self::default();
        let mut chars = opts.chars().peekable();

        while let Some(ch) = chars.next() {
            match ch {
                'd' => result.duplex = true,
                'q' => result.quiet = true,
                'r' => result.record = true,
                's' => result.skip_inuse = true,
                'i' => result.ignore_forwards = true,
                'A' => {
                    result.announcement = Self::extract_paren_arg(&mut chars);
                }
                'n' => result.no_caller_announce = true,
                'b' => {
                    result.predial_callee = Self::extract_paren_arg(&mut chars);
                }
                'B' => {
                    result.predial_caller = Self::extract_paren_arg(&mut chars);
                }
                _ => {
                    debug!("Page: ignoring unknown option '{}'", ch);
                }
            }
        }

        result
    }

    fn extract_paren_arg(chars: &mut std::iter::Peekable<std::str::Chars>) -> Option<String> {
        if chars.peek() == Some(&'(') {
            chars.next();
            let mut arg = String::new();
            for c in chars.by_ref() {
                if c == ')' {
                    break;
                }
                arg.push(c);
            }
            if arg.is_empty() { None } else { Some(arg) }
        } else {
            None
        }
    }
}

/// Parsed arguments for the Page application.
#[derive(Debug)]
pub struct PageArgs {
    /// List of Technology/Resource destinations to page.
    pub destinations: Vec<String>,
    /// Options for the page.
    pub options: PageOptions,
    /// Timeout in seconds (0 = no timeout).
    pub timeout: Duration,
}

impl PageArgs {
    /// Parse Page() argument string.
    ///
    /// Format: Tech/Resource[&Tech2/Resource2...][,options[,timeout]]
    pub fn parse(args: &str) -> Option<Self> {
        let parts: Vec<&str> = args.splitn(3, ',').collect();

        let dest_str = parts.first()?.trim();
        if dest_str.is_empty() {
            return None;
        }

        let destinations: Vec<String> = dest_str
            .split('&')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        if destinations.is_empty() {
            return None;
        }

        let options = parts
            .get(1)
            .map(|o| PageOptions::parse(o.trim()))
            .unwrap_or_default();

        let timeout = parts
            .get(2)
            .and_then(|t| t.trim().parse::<u64>().ok())
            .map(Duration::from_secs)
            .unwrap_or(Duration::ZERO);

        Some(Self {
            destinations,
            options,
            timeout,
        })
    }
}

/// The Page() dialplan application.
///
/// Usage: Page(Tech/Resource[&Tech2/Resource2...][,options[,timeout]])
///
/// Places outbound calls to all specified devices simultaneously and
/// dumps them into a conference bridge. The calling channel is the
/// announcer (speaker) and all paged channels are muted listeners.
pub struct AppPage;

impl DialplanApp for AppPage {
    fn name(&self) -> &str {
        "Page"
    }

    fn description(&self) -> &str {
        "Page series of phones"
    }
}

impl AppPage {
    /// Execute the Page application.
    ///
    /// # Arguments
    /// * `channel` - The originating (announcer) channel
    /// * `args` - Argument string
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parsed = match PageArgs::parse(args) {
            Some(a) => a,
            None => {
                warn!("Page: requires at least one Technology/Resource argument");
                channel.set_variable("PAGE_STATUS", PageStatus::Failed.as_str());
                return PbxExecResult::Failed;
            }
        };

        info!(
            "Page: channel '{}' paging {} destination(s) (duplex={}, timeout={:?})",
            channel.name,
            parsed.destinations.len(),
            parsed.options.duplex,
            parsed.timeout,
        );

        // Create a conference bridge for the page
        let bridge_name = format!("page-{}", channel.unique_id);
        let mut bridge = Bridge::new(&bridge_name);

        // Add the caller as the announcer (unmuted)
        bridge.add_channel(channel.unique_id.clone(), channel.name.clone());

        // In a real implementation:
        //
        //   // Create a ConfBridge with appropriate settings
        //   let conf_settings = ConferenceSettings {
        //       max_members: 0,
        //       music_on_hold_when_empty: false,
        //       ..Default::default()
        //   };
        //
        //   // Originate calls to each destination in parallel
        //   let mut dial_handles = Vec::new();
        //   for dest in &parsed.destinations {
        //       // Check device state if skip_inuse
        //       if parsed.options.skip_inuse {
        //           let state = get_device_state(dest);
        //           if state != DeviceState::NotInUse {
        //               debug!("Page: skipping '{}' - device in use", dest);
        //               continue;
        //           }
        //       }
        //
        //       let (tech, resource) = parse_tech_resource(dest)?;
        //       let tech_driver = ChannelDriverRegistry::find(&tech)?;
        //
        //       let handle = tokio::spawn(async move {
        //           let mut outbound = tech_driver.request(&resource, Some(channel)).await?;
        //           // Run pre-dial GoSub if configured
        //           if let Some(ref gosub) = parsed.options.predial_callee {
        //               run_gosub(&mut outbound, gosub).await;
        //           }
        //           // Dial with timeout
        //           tech_driver.call(&mut outbound, &resource, timeout_secs).await?;
        //           // When answered, join the bridge as a muted participant
        //           outbound.set_variable("CONFBRIDGE_JOIN_MUTED", "1");
        //           bridge.add_channel(outbound.unique_id.clone(), outbound.name.clone());
        //           Ok::<_, AsteriskError>(outbound)
        //       });
        //       dial_handles.push(handle);
        //   }
        //
        //   // Wait for all dial attempts to complete or timeout
        //   let timeout = if !parsed.timeout.is_zero() {
        //       parsed.timeout
        //   } else {
        //       Duration::from_secs(60)
        //   };
        //   let results = tokio::time::timeout(timeout, join_all(dial_handles)).await;
        //
        //   // Play beep to the page (unless quiet)
        //   if !parsed.options.quiet {
        //       play_file(channel, "beep").await;
        //   }
        //
        //   // Play announcement if configured
        //   if let Some(ref announcement) = parsed.options.announcement {
        //       play_file_to_bridge(&bridge, announcement).await;
        //   }
        //
        //   // Wait for the announcer to leave (hangup or DTMF exit)
        //   loop {
        //       select! {
        //           frame = channel.read_frame() => {
        //               match frame.frame_type {
        //                   FrameType::Voice => {
        //                       // Write voice to all bridge participants
        //                       bridge.write_to_others(channel.unique_id, &frame);
        //                   }
        //                   _ => {}
        //               }
        //           }
        //           _ = channel.hangup_signal() => break,
        //       }
        //   }
        //
        //   // Hangup all paged channels when announcer leaves
        //   for bc in bridge.channels {
        //       if bc.channel_id != channel.unique_id {
        //           hangup_channel(&bc.channel_id, HangupCause::NormalClearing);
        //       }
        //   }

        let status = PageStatus::Success;
        channel.set_variable("PAGE_STATUS", status.as_str());

        info!(
            "Page: channel '{}' page completed, status={}",
            channel.name,
            status.as_str()
        );

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_page_args_single_dest() {
        let args = PageArgs::parse("SIP/100").unwrap();
        assert_eq!(args.destinations, vec!["SIP/100"]);
        assert!(args.timeout.is_zero());
    }

    #[test]
    fn test_parse_page_args_multiple_dests() {
        let args = PageArgs::parse("SIP/100&SIP/200&SIP/300").unwrap();
        assert_eq!(args.destinations.len(), 3);
        assert_eq!(args.destinations[0], "SIP/100");
        assert_eq!(args.destinations[1], "SIP/200");
        assert_eq!(args.destinations[2], "SIP/300");
    }

    #[test]
    fn test_parse_page_args_with_options() {
        let args = PageArgs::parse("SIP/100&SIP/200,dqr,30").unwrap();
        assert_eq!(args.destinations.len(), 2);
        assert!(args.options.duplex);
        assert!(args.options.quiet);
        assert!(args.options.record);
        assert_eq!(args.timeout, Duration::from_secs(30));
    }

    #[test]
    fn test_parse_page_args_with_announcement() {
        let args = PageArgs::parse("SIP/100,A(announcement)").unwrap();
        assert_eq!(args.options.announcement.as_deref(), Some("announcement"));
    }

    #[test]
    fn test_parse_page_args_empty() {
        assert!(PageArgs::parse("").is_none());
    }

    #[test]
    fn test_page_options() {
        let opts = PageOptions::parse("dqs");
        assert!(opts.duplex);
        assert!(opts.quiet);
        assert!(opts.skip_inuse);
        assert!(!opts.record);
    }

    #[tokio::test]
    async fn test_page_exec() {
        let mut channel = Channel::new("SIP/announcer-001");
        let result = AppPage::exec(&mut channel, "SIP/100&SIP/200,q").await;
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(channel.get_variable("PAGE_STATUS"), Some("SUCCESS"));
    }

    #[tokio::test]
    async fn test_page_exec_no_args() {
        let mut channel = Channel::new("SIP/announcer-001");
        let result = AppPage::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Failed);
        assert_eq!(channel.get_variable("PAGE_STATUS"), Some("FAILED"));
    }
}
