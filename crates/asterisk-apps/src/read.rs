//! Read application - reads DTMF digits from a caller.
//!
//! Port of app_read.c from Asterisk C. Plays a prompt file and collects
//! DTMF digits from the caller, storing them in a channel variable.
//! Supports configurable max digits, terminators, retries, and timeouts.

use crate::playback::AppPlayback;
use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::{Channel, ChannelDriver};
use asterisk_types::{ChannelState, Frame};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Gate-compatible defaults when the dialplan leaves a timeout at zero.
const DEFAULT_OVERALL_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_DIGIT_TIMEOUT: Duration = Duration::from_secs(5);

/// Read status set as the READSTATUS channel variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadStatus {
    Ok,
    Error,
    Hangup,
    Interrupted,
    Skipped,
    Timeout,
}

impl ReadStatus {
    /// String representation for the READSTATUS variable.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Error => "ERROR",
            Self::Hangup => "HANGUP",
            Self::Interrupted => "INTERRUPTED",
            Self::Skipped => "SKIPPED",
            Self::Timeout => "TIMEOUT",
        }
    }
}

/// Options for the Read application.
#[derive(Debug, Clone)]
pub struct ReadOptions {
    /// Skip if the channel is not answered.
    pub skip: bool,
    /// Play filename as an indication tone.
    pub indication: bool,
    /// Read digits even if the line is not up.
    pub noanswer: bool,
    /// Terminator digit(s). Default is "#".
    pub terminator: String,
    /// If true, keep the terminator as part of digits when it's
    /// the only digit entered.
    pub keep_terminator: bool,
}

impl Default for ReadOptions {
    fn default() -> Self {
        Self {
            skip: false,
            indication: false,
            noanswer: false,
            terminator: "#".to_string(),
            keep_terminator: false,
        }
    }
}

impl ReadOptions {
    /// Parse the options string.
    ///
    /// Options: s=skip, i=indication, n=noanswer, t(chars)=terminator, e=keep_terminator
    pub fn parse(opts: &str) -> Self {
        let mut result = Self::default();
        let mut chars = opts.chars().peekable();
        while let Some(ch) = chars.next() {
            match ch {
                's' => result.skip = true,
                'i' => result.indication = true,
                'n' => result.noanswer = true,
                'e' => result.keep_terminator = true,
                't' => {
                    // The terminator option can have argument chars following it
                    // In the C code this is handled via OPT_ARG_TERMINATOR
                    // For simplicity, if 't' is followed by '(' we read until ')'
                    if chars.peek() == Some(&'(') {
                        chars.next(); // consume '('
                        let mut term = String::new();
                        for c in chars.by_ref() {
                            if c == ')' {
                                break;
                            }
                            term.push(c);
                        }
                        result.terminator = term;
                    } else {
                        // No argument means empty terminator (no termination by digit)
                        result.terminator.clear();
                    }
                }
                _ => {
                    debug!("Read: ignoring unknown option '{}'", ch);
                }
            }
        }
        result
    }
}

/// Parsed arguments for the Read application.
#[derive(Debug)]
pub struct ReadArgs {
    /// Variable name to store the result in.
    pub variable: String,
    /// Prompt filename(s) separated by '&'.
    pub filenames: Vec<String>,
    /// Maximum number of digits to read (0 = no limit, wait for #).
    pub max_digits: u32,
    /// Options.
    pub options: ReadOptions,
    /// Number of attempts (default 1).
    pub attempts: u32,
    /// Absolute input timeout in seconds (0 = 10 seconds).
    pub timeout: Duration,
    /// Timeout between accepted digits in seconds (0 = 5 seconds).
    pub digit_timeout: Duration,
}

impl ReadArgs {
    /// Parse the Read() argument string.
    ///
    /// Format: `variable[,filename[,maxdigits[,options[,attempts[,timeout[,digit_timeout]]]]]]`
    pub fn parse(args: &str) -> Option<Self> {
        let parts: Vec<&str> = args.splitn(7, ',').collect();

        let variable = parts.first()?.trim().to_string();
        if variable.is_empty() {
            return None;
        }

        let filenames = parts.get(1).map_or_else(Vec::new, |f| {
            f.trim()
                .split('&')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect()
        });

        let max_digits = parts
            .get(2)
            .and_then(|m| {
                let trimmed = m.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    trimmed.parse::<u32>().ok()
                }
            })
            .unwrap_or(0);
        // Clamp to 255 as in C code
        let max_digits = max_digits.min(255);

        let options = parts
            .get(3)
            .map(|o| ReadOptions::parse(o.trim()))
            .unwrap_or_default();

        let attempts = parts
            .get(4)
            .and_then(|a| {
                let trimmed = a.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    trimmed.parse::<u32>().ok()
                }
            })
            .unwrap_or(1)
            .max(1);

        let timeout = parse_timeout(parts.get(5));
        let digit_timeout = parse_timeout(parts.get(6));

        Some(Self {
            variable,
            filenames,
            max_digits,
            options,
            attempts,
            timeout,
            digit_timeout,
        })
    }
}

fn parse_timeout(value: Option<&&str>) -> Duration {
    value
        .and_then(|value| value.trim().parse::<f64>().ok())
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .map(Duration::from_secs_f64)
        .unwrap_or(Duration::ZERO)
}

/// The Read() dialplan application.
///
/// Reads a '#'-terminated string of digits from the user into a channel
/// variable. Plays an optional prompt, supports configurable max digits,
/// retries, and timeout.
///
/// Usage: Read(variable[,filename[,maxdigits[,options[,attempts[,timeout[,digit_timeout]]]]]])
///
/// Sets READSTATUS channel variable (OK, ERROR, HANGUP, INTERRUPTED, SKIPPED, TIMEOUT).
pub struct AppRead;

impl DialplanApp for AppRead {
    fn name(&self) -> &str {
        "Read"
    }

    fn description(&self) -> &str {
        "Read a variable"
    }
}

impl AppRead {
    /// Execute the Read application.
    ///
    /// # Arguments
    /// * `channel` - The channel to read digits from
    /// * `args` - Argument string
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parsed = match ReadArgs::parse(args) {
            Some(a) => a,
            None => {
                warn!("Read: requires an argument (variable)");
                channel.set_variable("READSTATUS", ReadStatus::Error.as_str());
                return PbxExecResult::Success;
            }
        };

        // Skip if channel is not answered and 's' option is set
        if parsed.options.skip && channel.state != ChannelState::Up {
            debug!("Read: skipping - channel not answered and 's' option set");
            channel.set_variable(&parsed.variable, "");
            channel.set_variable("READSTATUS", ReadStatus::Skipped.as_str());
            return PbxExecResult::Success;
        }

        // Answer the channel if needed (unless 'n' option). Use answer(), not
        // a raw state assignment, so the SIP handler is notified and can send
        // its 200 OK.
        if !parsed.options.noanswer && channel.state != ChannelState::Up {
            debug!("Read: answering channel before reading");
            channel.answer();
        }

        let tech = channel.name.split('/').next().unwrap_or("");
        let driver = match asterisk_core::channel::tech_registry::TECH_REGISTRY.find(tech) {
            Some(driver) => driver,
            None => {
                warn!(
                    "Read: channel '{}' has no '{}' media driver",
                    channel.name, tech
                );
                channel.set_variable(&parsed.variable, "");
                channel.set_variable("READSTATUS", ReadStatus::Error.as_str());
                return PbxExecResult::Success;
            }
        };

        let overall_timeout = if parsed.timeout.is_zero() {
            DEFAULT_OVERALL_TIMEOUT
        } else {
            parsed.timeout
        };
        let digit_timeout = if parsed.digit_timeout.is_zero() {
            DEFAULT_DIGIT_TIMEOUT
        } else {
            parsed.digit_timeout
        };

        info!(
            "Read: reading up to {} digits into '{}' from channel '{}' (attempts={}, timeout={:?})",
            if parsed.max_digits == 0 {
                "unlimited".to_string()
            } else {
                parsed.max_digits.to_string()
            },
            parsed.variable,
            channel.name,
            parsed.attempts,
            overall_timeout,
        );

        let mut digits = String::new();
        let mut status = ReadStatus::Timeout;

        // Attempt loop
        for attempt in 0..parsed.attempts {
            digits.clear();
            if channel_has_hung_up(channel) {
                status = ReadStatus::Hangup;
                break;
            }

            debug!("Read: attempt {}/{}", attempt + 1, parsed.attempts);

            if !parsed.filenames.is_empty() {
                if parsed.options.indication {
                    debug!(
                        "Read: indication option is not distinct yet; playing prompt as audio"
                    );
                }
                let prompt = parsed.filenames.join("&");
                match AppPlayback::exec(channel, &prompt).await {
                    PbxExecResult::Success => {}
                    PbxExecResult::Hangup => {
                        status = ReadStatus::Hangup;
                        break;
                    }
                    PbxExecResult::Failed => {
                        status = ReadStatus::Error;
                        break;
                    }
                }
            }

            let result = collect_attempt(
                channel,
                driver.clone(),
                &parsed,
                overall_timeout,
                digit_timeout,
            )
            .await;
            digits = result.0;
            status = result.1;

            if status != ReadStatus::Timeout {
                break;
            }
        }

        // Set the result variable
        channel.set_variable(&parsed.variable, &digits);
        channel.set_variable("READSTATUS", status.as_str());

        debug!(
            "Read: result variable '{}' updated (digit count redacted), READSTATUS = {}",
            parsed.variable,
            status.as_str()
        );

        match status {
            ReadStatus::Hangup => PbxExecResult::Hangup,
            _ => PbxExecResult::Success,
        }
    }
}

fn channel_has_hung_up(channel: &Channel) -> bool {
    if channel.state == ChannelState::Down || channel.check_hangup() {
        return true;
    }

    asterisk_core::channel_store::find_by_name(&channel.name).is_some_and(|stored| {
        let stored = stored.lock();
        stored.state == ChannelState::Down || stored.check_hangup()
    })
}

/// Collect one Read attempt from receiver-side media frames.
///
/// The overall deadline never moves. Once a non-terminator digit arrives, an
/// independent inter-digit deadline is armed and reset only by another
/// accepted digit; voice traffic therefore cannot keep a partial PIN alive.
async fn collect_attempt(
    channel: &mut Channel,
    driver: Arc<dyn ChannelDriver>,
    parsed: &ReadArgs,
    overall_timeout: Duration,
    digit_timeout: Duration,
) -> (String, ReadStatus) {
    let mut digits = String::new();
    let overall_deadline = tokio::time::Instant::now() + overall_timeout;
    let mut digit_deadline = None;
    let max_digits = if parsed.max_digits == 0 {
        255
    } else {
        parsed.max_digits as usize
    };

    loop {
        if channel_has_hung_up(channel) {
            return (digits, ReadStatus::Hangup);
        }

        let deadline = match digit_deadline {
            Some(inter_digit) if inter_digit < overall_deadline => inter_digit,
            _ => overall_deadline,
        };
        if tokio::time::Instant::now() >= deadline {
            return (digits, ReadStatus::Timeout);
        }

        match tokio::time::timeout_at(deadline, driver.read_frame(channel)).await {
            Ok(Ok(Frame::DtmfEnd { digit, .. })) => {
                if parsed.options.terminator.contains(digit) {
                    if digits.is_empty() && parsed.options.keep_terminator {
                        digits.push(digit);
                    }
                    return (digits, ReadStatus::Ok);
                }

                digits.push(digit);
                if digits.len() >= max_digits {
                    return (digits, ReadStatus::Ok);
                }
                digit_deadline = Some(tokio::time::Instant::now() + digit_timeout);
            }
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                warn!(
                    "Read: media read failed on channel '{}': {}",
                    channel.name, error
                );
                let status = if channel_has_hung_up(channel) {
                    ReadStatus::Hangup
                } else {
                    ReadStatus::Error
                };
                return (digits, status);
            }
            Err(_) => {
                let status = if channel_has_hung_up(channel) {
                    ReadStatus::Hangup
                } else {
                    ReadStatus::Timeout
                };
                return (digits, status);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use asterisk_types::{AsteriskError, AsteriskResult};
    use std::sync::atomic::{AtomicU32, Ordering};
    use tokio::sync::{mpsc, Mutex};

    static TEST_DRIVER_ID: AtomicU32 = AtomicU32::new(1);

    #[derive(Debug)]
    struct MockReadDriver {
        name: String,
        frames: Mutex<mpsc::Receiver<Frame>>,
    }

    #[async_trait::async_trait]
    impl ChannelDriver for MockReadDriver {
        fn name(&self) -> &str {
            &self.name
        }

        fn description(&self) -> &str {
            "Read test driver"
        }

        async fn request(
            &self,
            dest: &str,
            _caller: Option<&Channel>,
        ) -> AsteriskResult<Channel> {
            Ok(Channel::new(format!("{}/{}", self.name, dest)))
        }

        async fn call(
            &self,
            _channel: &mut Channel,
            _dest: &str,
            _timeout: i32,
        ) -> AsteriskResult<()> {
            Ok(())
        }

        async fn hangup(&self, channel: &mut Channel) -> AsteriskResult<()> {
            channel.set_state(ChannelState::Down);
            Ok(())
        }

        async fn answer(&self, channel: &mut Channel) -> AsteriskResult<()> {
            channel.answer();
            Ok(())
        }

        async fn read_frame(&self, _channel: &mut Channel) -> AsteriskResult<Frame> {
            self.frames
                .lock()
                .await
                .recv()
                .await
                .ok_or_else(|| AsteriskError::Hangup("test frame stream closed".to_string()))
        }

        async fn write_frame(
            &self,
            _channel: &mut Channel,
            _frame: &Frame,
        ) -> AsteriskResult<()> {
            Ok(())
        }
    }

    fn install_mock_driver() -> (String, mpsc::Sender<Frame>) {
        let id = TEST_DRIVER_ID.fetch_add(1, Ordering::Relaxed);
        let name = format!("READTEST{}", id);
        let (sender, receiver) = mpsc::channel(32);
        let driver = Arc::new(MockReadDriver {
            name: name.clone(),
            frames: Mutex::new(receiver),
        });
        asterisk_core::channel::tech_registry::TECH_REGISTRY.register(driver);
        (name, sender)
    }

    fn up_channel(tech: &str) -> Channel {
        let mut channel = Channel::new(format!("{}/call", tech));
        channel.state = ChannelState::Up;
        channel
    }

    async fn send_digits(sender: &mpsc::Sender<Frame>, digits: &str) {
        for digit in digits.chars() {
            sender.send(Frame::dtmf_end(digit, 100)).await.unwrap();
        }
    }

    #[test]
    fn test_parse_read_args_minimal() {
        let args = ReadArgs::parse("RESULT").unwrap();
        assert_eq!(args.variable, "RESULT");
        assert!(args.filenames.is_empty());
        assert_eq!(args.max_digits, 0);
        assert_eq!(args.attempts, 1);
        assert_eq!(args.timeout, Duration::ZERO);
        assert_eq!(args.digit_timeout, Duration::ZERO);
    }

    #[test]
    fn test_parse_read_args_full() {
        let args = ReadArgs::parse("DIGITS,prompt&beep,4,s,3,10,5").unwrap();
        assert_eq!(args.variable, "DIGITS");
        assert_eq!(args.filenames, vec!["prompt", "beep"]);
        assert_eq!(args.max_digits, 4);
        assert!(args.options.skip);
        assert_eq!(args.attempts, 3);
        assert_eq!(args.timeout, Duration::from_secs(10));
        assert_eq!(args.digit_timeout, Duration::from_secs(5));
    }

    #[test]
    fn test_parse_read_args_empty() {
        assert!(ReadArgs::parse("").is_none());
    }

    #[test]
    fn test_invalid_timeouts_use_defaults_without_panicking() {
        let args = ReadArgs::parse("DIGITS,,,,,-1,NaN").unwrap();
        assert_eq!(args.timeout, Duration::ZERO);
        assert_eq!(args.digit_timeout, Duration::ZERO);
    }

    #[test]
    fn test_parse_options() {
        let opts = ReadOptions::parse("sin");
        assert!(opts.skip);
        assert!(opts.indication);
        assert!(opts.noanswer);
    }

    #[test]
    fn test_parse_options_terminator() {
        let opts = ReadOptions::parse("t(*)");
        assert_eq!(opts.terminator, "*");
    }

    #[test]
    fn test_parse_options_empty_terminator() {
        let opts = ReadOptions::parse("t");
        assert_eq!(opts.terminator, "");
    }

    #[test]
    fn test_max_digits_clamp() {
        let args = ReadArgs::parse("VAR,prompt,999").unwrap();
        assert_eq!(args.max_digits, 255);
    }

    #[tokio::test]
    async fn test_read_skip_not_answered() {
        let mut channel = Channel::new("SIP/test-001");
        // Channel is Down by default
        let result = AppRead::exec(&mut channel, "RESULT,prompt,4,s").await;
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(channel.get_variable("READSTATUS"), Some("SKIPPED"));
    }

    #[tokio::test]
    async fn read_stops_at_fixed_digit_count() {
        let (tech, sender) = install_mock_driver();
        send_digits(&sender, "123456").await;
        let mut channel = up_channel(&tech);

        let result = AppRead::exec(&mut channel, "PIN,,6,,1,1,0.2").await;

        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(channel.get_variable("PIN"), Some("123456"));
        assert_eq!(channel.get_variable("READSTATUS"), Some("OK"));
        asterisk_core::channel::tech_registry::TECH_REGISTRY.unregister(&tech);
    }

    #[tokio::test]
    async fn read_hash_terminates_without_storing_terminator() {
        let (tech, sender) = install_mock_driver();
        send_digits(&sender, "12#").await;
        let mut channel = up_channel(&tech);

        AppRead::exec(&mut channel, "PIN,,6,,1,1,0.2").await;

        assert_eq!(channel.get_variable("PIN"), Some("12"));
        assert_eq!(channel.get_variable("READSTATUS"), Some("OK"));
        asterisk_core::channel::tech_registry::TECH_REGISTRY.unregister(&tech);
    }

    #[tokio::test]
    async fn read_enforces_overall_timeout_before_first_digit() {
        let (tech, _sender) = install_mock_driver();
        let mut channel = up_channel(&tech);

        AppRead::exec(&mut channel, "PIN,,6,,1,0.03,1").await;

        assert_eq!(channel.get_variable("PIN"), Some(""));
        assert_eq!(channel.get_variable("READSTATUS"), Some("TIMEOUT"));
        asterisk_core::channel::tech_registry::TECH_REGISTRY.unregister(&tech);
    }

    #[tokio::test]
    async fn read_enforces_inter_digit_timeout_after_partial_pin() {
        let (tech, sender) = install_mock_driver();
        send_digits(&sender, "1").await;
        let mut channel = up_channel(&tech);

        AppRead::exec(&mut channel, "PIN,,6,,1,1,0.03").await;

        assert_eq!(channel.get_variable("PIN"), Some("1"));
        assert_eq!(channel.get_variable("READSTATUS"), Some("TIMEOUT"));
        asterisk_core::channel::tech_registry::TECH_REGISTRY.unregister(&tech);
    }

    #[tokio::test]
    async fn read_is_registered_as_real_async_adapter() {
        crate::adapter::register_all_apps();
        let app = asterisk_core::pbx::app_registry::APP_REGISTRY
            .find("Read")
            .expect("Read must be registered");
        let mut channel = up_channel("NO_READ_DRIVER");

        app.execute(&mut channel, "PIN,,6,,1,0.01,0.01")
            .await;

        assert_eq!(channel.get_variable("READSTATUS"), Some("ERROR"));
    }
}
