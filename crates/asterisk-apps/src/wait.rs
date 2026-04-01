//! Wait applications - pause dialplan execution.
//!
//! Ports of app_wait.c and app_waituntil.c from Asterisk C.
//! Provides Wait(), WaitExten(), WaitDigit(), and WaitUntil() applications.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use asterisk_types::ChannelState;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

/// The Wait() dialplan application.
///
/// Pauses dialplan execution for a specified number of seconds.
/// The wait is interruptible by channel hangup.
///
/// Usage: Wait(seconds)
///
/// The seconds argument can be a floating point value.
pub struct AppWait;

impl DialplanApp for AppWait {
    fn name(&self) -> &str {
        "Wait"
    }

    fn description(&self) -> &str {
        "Waits for some time"
    }
}

impl AppWait {
    /// Execute the Wait application.
    ///
    /// # Arguments
    /// * `channel` - The current channel
    /// * `args` - Number of seconds to wait (can be fractional)
    pub async fn exec(channel: &Channel, args: &str) -> PbxExecResult {
        let seconds: f64 = match args.trim().parse() {
            Ok(s) if s > 0.0 => s,
            _ => {
                if !args.trim().is_empty() {
                    warn!("Wait: invalid argument '{}', not waiting", args);
                }
                return PbxExecResult::Success;
            }
        };

        let duration = Duration::from_secs_f64(seconds);
        debug!(
            "Wait: waiting {:.3}s on channel '{}'",
            seconds, channel.name
        );

        if channel.state == ChannelState::Down {
            return PbxExecResult::Hangup;
        }

        // Sleep in small intervals, checking the channel store for hangup
        // between each interval. This allows Wait to be interrupted when the
        // other side of a Local channel pair hangs up (which updates the
        // channel store copy but not our local Channel object).
        let poll_interval = Duration::from_millis(100);
        let start = std::time::Instant::now();
        let chan_name = channel.name.clone();

        while start.elapsed() < duration {
            let remaining = duration.saturating_sub(start.elapsed());
            let sleep_time = remaining.min(poll_interval);
            tokio::time::sleep(sleep_time).await;

            // Check our own channel state
            if channel.state == ChannelState::Down || channel.check_hangup() {
                debug!("Wait: channel '{}' hung up during wait", chan_name);
                return PbxExecResult::Hangup;
            }

            // Check the channel store (handles Local channel peer hangup)
            if let Some(store_chan) = asterisk_core::channel_store::find_by_name(&chan_name) {
                let ch = store_chan.lock();
                if ch.state == ChannelState::Down || ch.check_hangup() {
                    debug!("Wait: channel '{}' hung up in store during wait", chan_name);
                    return PbxExecResult::Hangup;
                }
            } else {
                // Channel no longer in store — it was deregistered (hung up)
                debug!("Wait: channel '{}' no longer in store, treating as hangup", chan_name);
                return PbxExecResult::Hangup;
            }
        }

        PbxExecResult::Success
    }
}

/// The WaitExten() dialplan application.
///
/// Waits for the caller to enter a new extension. If no extension
/// is entered within the timeout, dialplan execution continues.
///
/// Usage: WaitExten([seconds])
///
/// If seconds is not specified, the default extension timeout is used.
pub struct AppWaitExten;

impl DialplanApp for AppWaitExten {
    fn name(&self) -> &str {
        "WaitExten"
    }

    fn description(&self) -> &str {
        "Waits for an extension to be entered"
    }
}

impl AppWaitExten {
    /// Execute the WaitExten application.
    ///
    /// # Arguments
    /// * `channel` - The current channel
    /// * `args` - Optional timeout in seconds
    pub async fn exec(channel: &Channel, args: &str) -> PbxExecResult {
        let timeout = if args.trim().is_empty() {
            // Default timeout (typically 10 seconds in Asterisk)
            Duration::from_secs(10)
        } else {
            match args.trim().parse::<f64>() {
                Ok(s) if s > 0.0 => Duration::from_secs_f64(s),
                _ => {
                    warn!("WaitExten: invalid timeout '{}', using default", args);
                    Duration::from_secs(10)
                }
            }
        };

        debug!(
            "WaitExten: waiting {:.3}s for extension input on channel '{}'",
            timeout.as_secs_f64(),
            channel.name
        );

        if channel.state == ChannelState::Down {
            return PbxExecResult::Hangup;
        }

        // In a full implementation, we'd wait for DTMF digits and match
        // them against the current dialplan context:
        //
        //   let result = pbx_waitexten(channel, timeout).await;
        //   match result {
        //       WaitResult::Extension(exten) => {
        //           // Extension matched, PBX core will handle the jump
        //           PbxExecResult::Success
        //       }
        //       WaitResult::Timeout => PbxExecResult::Success,
        //       WaitResult::Hangup => PbxExecResult::Hangup,
        //   }

        tokio::time::sleep(timeout).await;

        PbxExecResult::Success
    }
}

/// The WaitDigit() dialplan application.
///
/// Waits for the caller to press a single DTMF digit.
/// The received digit is stored in the WAITDIGITSTATUS variable.
///
/// Usage: WaitDigit([seconds])
pub struct AppWaitDigit;

impl DialplanApp for AppWaitDigit {
    fn name(&self) -> &str {
        "WaitDigit"
    }

    fn description(&self) -> &str {
        "Waits for a digit to be entered"
    }
}

impl AppWaitDigit {
    /// Execute the WaitDigit application.
    ///
    /// # Arguments
    /// * `channel` - The current channel
    /// * `args` - Optional timeout in seconds
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let timeout = if args.trim().is_empty() {
            Duration::ZERO // Infinite wait
        } else {
            match args.trim().parse::<f64>() {
                Ok(s) if s > 0.0 => Duration::from_secs_f64(s),
                _ => {
                    warn!("WaitDigit: invalid timeout '{}'", args);
                    Duration::ZERO
                }
            }
        };

        debug!(
            "WaitDigit: waiting for digit on channel '{}' (timeout={:?})",
            channel.name, timeout
        );

        if channel.state == ChannelState::Down {
            channel.set_variable("WAITDIGITSTATUS", "HANGUP");
            return PbxExecResult::Hangup;
        }

        // In a full implementation:
        //
        //   let result = channel.wait_for_digit(timeout).await;
        //   match result {
        //       Ok(Some(digit)) => {
        //           channel.set_variable("WAITDIGITSTATUS", &digit.to_string());
        //           PbxExecResult::Success
        //       }
        //       Ok(None) => {
        //           // Timeout
        //           channel.set_variable("WAITDIGITSTATUS", "");
        //           PbxExecResult::Success
        //       }
        //       Err(_) => {
        //           channel.set_variable("WAITDIGITSTATUS", "HANGUP");
        //           PbxExecResult::Hangup
        //       }
        //   }

        // Wait for the full duration (no real DTMF detection yet).
        if !timeout.is_zero() {
            tokio::time::sleep(timeout).await;
        }
        channel.set_variable("WAITDIGITSTATUS", "");

        PbxExecResult::Success
    }
}

/// WaitUntil status set as WAITUNTILSTATUS channel variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitUntilStatus {
    /// Wait succeeded -- the target time was reached.
    Ok,
    /// Invalid argument provided.
    Failure,
    /// Channel hung up before the target time.
    Hangup,
    /// The specified time had already passed.
    Past,
}

impl WaitUntilStatus {
    /// String representation for the WAITUNTILSTATUS variable.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Failure => "FAILURE",
            Self::Hangup => "HANGUP",
            Self::Past => "PAST",
        }
    }
}

/// The WaitUntil() dialplan application.
///
/// Waits (sleeps) until the current time reaches the given Unix epoch.
///
/// Usage: WaitUntil(epoch)
///
/// Sets WAITUNTILSTATUS channel variable (OK, FAILURE, HANGUP, PAST).
pub struct AppWaitUntil;

impl DialplanApp for AppWaitUntil {
    fn name(&self) -> &str {
        "WaitUntil"
    }

    fn description(&self) -> &str {
        "Wait (sleep) until the current time is the given epoch"
    }
}

impl AppWaitUntil {
    /// Execute the WaitUntil application.
    ///
    /// # Arguments
    /// * `channel` - The current channel
    /// * `args` - Unix epoch timestamp (can be floating point)
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        if args.trim().is_empty() {
            warn!("WaitUntil: requires an argument (epoch)");
            channel.set_variable("WAITUNTILSTATUS", WaitUntilStatus::Failure.as_str());
            return PbxExecResult::Success;
        }

        let epoch: f64 = match args.trim().parse() {
            Ok(v) => v,
            Err(_) => {
                warn!("WaitUntil: called with non-numeric argument '{}'", args);
                channel.set_variable("WAITUNTILSTATUS", WaitUntilStatus::Failure.as_str());
                return PbxExecResult::Success;
            }
        };

        let target = Duration::from_secs_f64(epoch);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO);

        if target <= now {
            info!(
                "WaitUntil: target time {} already passed (now={})",
                epoch,
                now.as_secs_f64()
            );
            channel.set_variable("WAITUNTILSTATUS", WaitUntilStatus::Past.as_str());
            return PbxExecResult::Success;
        }

        let wait_duration = target - now;
        debug!(
            "WaitUntil: waiting {:.3}s until epoch {} on channel '{}'",
            wait_duration.as_secs_f64(),
            epoch,
            channel.name
        );

        if channel.state == ChannelState::Down {
            channel.set_variable("WAITUNTILSTATUS", WaitUntilStatus::Hangup.as_str());
            return PbxExecResult::Hangup;
        }

        // In a full implementation, we'd use safe_sleep that monitors for hangup:
        //
        //   match channel.safe_sleep(wait_duration).await {
        //       Ok(()) => {
        //           channel.set_variable("WAITUNTILSTATUS", "OK");
        //           PbxExecResult::Success
        //       }
        //       Err(_) => {
        //           channel.set_variable("WAITUNTILSTATUS", "HANGUP");
        //           PbxExecResult::Hangup
        //       }
        //   }

        tokio::time::sleep(wait_duration).await;

        channel.set_variable("WAITUNTILSTATUS", WaitUntilStatus::Ok.as_str());
        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wait_until_status_strings() {
        assert_eq!(WaitUntilStatus::Ok.as_str(), "OK");
        assert_eq!(WaitUntilStatus::Failure.as_str(), "FAILURE");
        assert_eq!(WaitUntilStatus::Hangup.as_str(), "HANGUP");
        assert_eq!(WaitUntilStatus::Past.as_str(), "PAST");
    }

    #[tokio::test]
    async fn test_wait_until_past() {
        let mut channel = Channel::new("SIP/test-001");
        channel.state = ChannelState::Up;
        // Epoch 0 is definitely in the past
        let result = AppWaitUntil::exec(&mut channel, "0").await;
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(channel.get_variable("WAITUNTILSTATUS"), Some("PAST"));
    }

    #[tokio::test]
    async fn test_wait_until_invalid() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppWaitUntil::exec(&mut channel, "notanumber").await;
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(channel.get_variable("WAITUNTILSTATUS"), Some("FAILURE"));
    }

    #[tokio::test]
    async fn test_wait_until_empty() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppWaitUntil::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(channel.get_variable("WAITUNTILSTATUS"), Some("FAILURE"));
    }

    #[tokio::test]
    async fn test_wait_hangup() {
        let channel = Channel::new("SIP/test-001");
        // Channel is Down by default
        let result = AppWait::exec(&channel, "5").await;
        assert_eq!(result, PbxExecResult::Hangup);
    }

    #[tokio::test]
    async fn test_wait_digit_hangup() {
        let mut channel = Channel::new("SIP/test-001");
        // Channel is Down by default
        let result = AppWaitDigit::exec(&mut channel, "5").await;
        assert_eq!(result, PbxExecResult::Hangup);
        assert_eq!(channel.get_variable("WAITDIGITSTATUS"), Some("HANGUP"));
    }
}
