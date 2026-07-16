//! Secret-safe PIN gate.
//!
//! Unlike `Read()`, this application never stores entered digits in a channel
//! variable. The dialplan and all event sinks see only `PINGATESTATUS`.

use crate::playback::AppPlayback;
use crate::{DialplanApp, PbxExecResult};
use asterisk_core::bridge::lifetime::BridgeLifetime;
use asterisk_core::channel::{Channel, ChannelDriver};
use asterisk_types::{ChannelState, Frame};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use subtle::ConstantTimeEq;
use tracing::{error, warn};
use zeroize::{Zeroize, Zeroizing};

pub const PIN_LENGTH: usize = 6;
const DEFAULT_DEADLINE: Duration = Duration::from_secs(10);
const DEFAULT_DIGIT_TIMEOUT: Duration = Duration::from_secs(5);

static PIN_SECRET: OnceLock<PinSecret> = OnceLock::new();

/// Fixed-length validated secret. Its bytes are never formatted or exposed.
pub struct PinSecret([u8; PIN_LENGTH]);

impl PinSecret {
    pub fn parse(mut bytes: Vec<u8>) -> Result<Self, String> {
        if bytes.last() == Some(&b'\n') {
            bytes.pop();
            if bytes.last() == Some(&b'\r') {
                bytes.pop();
            }
        }

        if bytes.len() != PIN_LENGTH || !bytes.iter().all(u8::is_ascii_digit) {
            bytes.zeroize();
            return Err(format!(
                "PIN secret must contain exactly {PIN_LENGTH} ASCII digits"
            ));
        }

        let mut secret = [0_u8; PIN_LENGTH];
        secret.copy_from_slice(&bytes);
        bytes.zeroize();
        Ok(Self(secret))
    }

    /// Compare exactly six positions and fold the entered length into the result.
    /// There is no data-dependent early return.
    fn matches(&self, entered: &[u8]) -> bool {
        let mut candidate = [0_u8; PIN_LENGTH];
        for (index, byte) in candidate.iter_mut().enumerate() {
            *byte = entered.get(index).copied().unwrap_or(0);
        }
        let length_matches = (entered.len() as u64).ct_eq(&(PIN_LENGTH as u64));
        let matches = candidate.ct_eq(&self.0) & length_matches;
        candidate.zeroize();
        matches.into()
    }
}

impl Drop for PinSecret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Install the startup-loaded secret exactly once.
pub fn install_pin_secret(secret: PinSecret) -> Result<(), String> {
    PIN_SECRET
        .set(secret)
        .map_err(|_| "PIN secret was already installed".to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PinGateStatus {
    Granted,
    Rejected,
    Timeout,
    Error,
}

impl PinGateStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Granted => "GRANTED",
            Self::Rejected => "REJECTED",
            Self::Timeout => "TIMEOUT",
            Self::Error => "ERROR",
        }
    }
}

#[derive(Debug)]
struct PinGateArgs {
    prompt: String,
    deadline: Duration,
    digit_timeout: Duration,
}

impl PinGateArgs {
    /// `PinGate(prompt[,absolute-deadline-seconds[,digit-timeout-seconds]])`
    fn parse(args: &str) -> Result<Self, String> {
        let mut parts = args.splitn(3, ',');
        let prompt = parts.next().unwrap_or_default().trim().to_string();
        let deadline = parse_positive_duration(parts.next(), DEFAULT_DEADLINE)?;
        let digit_timeout = parse_positive_duration(parts.next(), DEFAULT_DIGIT_TIMEOUT)?;
        Ok(Self {
            prompt,
            deadline,
            digit_timeout,
        })
    }
}

fn parse_positive_duration(value: Option<&str>, default: Duration) -> Result<Duration, String> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(default);
    };
    let seconds = value
        .parse::<f64>()
        .map_err(|_| "PinGate timeout must be a positive number".to_string())?;
    if !seconds.is_finite() || seconds <= 0.0 {
        return Err("PinGate timeout must be a positive number".to_string());
    }
    Ok(Duration::from_secs_f64(seconds))
}

pub struct AppPinGate;

impl DialplanApp for AppPinGate {
    fn name(&self) -> &str {
        "PinGate"
    }

    fn description(&self) -> &str {
        "Authenticate a caller without publishing entered digits"
    }
}

impl AppPinGate {
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parsed = match PinGateArgs::parse(args) {
            Ok(parsed) => parsed,
            Err(reason) => {
                error!(reason, "PinGate configuration is invalid; hanging up");
                return fail_closed(channel, PinGateStatus::Error);
            }
        };
        let Some(secret) = PIN_SECRET.get() else {
            error!("PinGate secret is unavailable; hanging up");
            return fail_closed(channel, PinGateStatus::Error);
        };

        if channel.state != ChannelState::Up {
            channel.answer();
        }

        // Arm only after answer. The timer cancels the same BridgeLifetime
        // token used by M2 media pumps; no media-plane socket/drop is involved.
        let lifetime = BridgeLifetime::new();
        let lifetime_registration = lifetime.register_channels([channel.name.clone()]);
        let deadline_lifetime = lifetime.clone();
        let deadline = parsed.deadline;
        let deadline_task = tokio::spawn(async move {
            tokio::time::sleep(deadline).await;
            deadline_lifetime.cancel();
        });

        let result = run_gate(channel, &parsed, secret, &lifetime).await;
        deadline_task.abort();
        drop(lifetime_registration);

        match result {
            PinGateStatus::Granted | PinGateStatus::Rejected => {
                channel.set_variable("PINGATESTATUS", result.as_str());
                PbxExecResult::Success
            }
            PinGateStatus::Timeout | PinGateStatus::Error => fail_closed(channel, result),
        }
    }
}

async fn run_gate(
    channel: &mut Channel,
    args: &PinGateArgs,
    secret: &PinSecret,
    lifetime: &BridgeLifetime,
) -> PinGateStatus {
    if !args.prompt.is_empty() {
        tokio::select! {
            _ = lifetime.cancelled() => return PinGateStatus::Timeout,
            playback = AppPlayback::exec(channel, &args.prompt) => {
                if playback != PbxExecResult::Success {
                    return PinGateStatus::Error;
                }
            }
        }
    }

    let tech = channel.name.split('/').next().unwrap_or_default();
    let Some(driver) = asterisk_core::channel::tech_registry::TECH_REGISTRY.find(tech) else {
        warn!(channel = %channel.name, technology = tech, "PinGate media driver is unavailable");
        return PinGateStatus::Error;
    };

    let entered = match collect_digits(channel, driver, args.digit_timeout, lifetime).await {
        Ok(entered) => entered,
        Err(status) => return status,
    };

    if secret.matches(&entered) {
        PinGateStatus::Granted
    } else {
        PinGateStatus::Rejected
    }
}

async fn collect_digits(
    channel: &mut Channel,
    driver: Arc<dyn ChannelDriver>,
    digit_timeout: Duration,
    lifetime: &BridgeLifetime,
) -> Result<Zeroizing<Vec<u8>>, PinGateStatus> {
    let mut entered = Zeroizing::new(Vec::with_capacity(PIN_LENGTH));
    let mut inter_digit_deadline = None;

    loop {
        if channel_has_hung_up(channel) {
            return Err(PinGateStatus::Error);
        }

        let read = async {
            if let Some(deadline) = inter_digit_deadline {
                tokio::time::timeout_at(deadline, driver.read_frame(channel))
                    .await
                    .map_err(|_| PinGateStatus::Rejected)?
            } else {
                driver.read_frame(channel).await
            }
            .map_err(|_| PinGateStatus::Error)
        };

        let frame = tokio::select! {
            _ = lifetime.cancelled() => return Err(PinGateStatus::Timeout),
            frame = read => frame?,
        };

        if let Frame::DtmfEnd { digit, .. } = frame {
            if digit == '#' {
                break;
            }
            if !digit.is_ascii_digit() {
                continue;
            }
            entered.push(digit as u8);
            if entered.len() == PIN_LENGTH {
                break;
            }
            inter_digit_deadline = Some(tokio::time::Instant::now() + digit_timeout);
        }
    }

    Ok(entered)
}

fn channel_has_hung_up(channel: &Channel) -> bool {
    channel.state == ChannelState::Down
        || channel.check_hangup()
        || asterisk_core::channel_store::find_by_name(&channel.name).is_some_and(|stored| {
            let stored = stored.lock();
            stored.state == ChannelState::Down || stored.check_hangup()
        })
}

fn fail_closed(channel: &mut Channel, status: PinGateStatus) -> PbxExecResult {
    channel.set_variable("PINGATESTATUS", status.as_str());
    channel.softhangup(asterisk_core::softhangup::AST_SOFTHANGUP_TIMEOUT);
    if let Some(stored) = asterisk_core::channel_store::find_by_name(&channel.name) {
        stored
            .lock()
            .softhangup(asterisk_core::softhangup::AST_SOFTHANGUP_TIMEOUT);
    }
    PbxExecResult::Hangup
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret() -> PinSecret {
        PinSecret::parse(b"246810\n".to_vec()).unwrap()
    }

    #[test]
    fn accepts_optional_single_line_ending() {
        assert!(PinSecret::parse(b"246810".to_vec()).is_ok());
        assert!(PinSecret::parse(b"246810\n".to_vec()).is_ok());
        assert!(PinSecret::parse(b"246810\r\n".to_vec()).is_ok());
    }

    #[test]
    fn rejects_missing_malformed_and_ambiguous_secrets() {
        assert!(PinSecret::parse(Vec::new()).is_err());
        assert!(PinSecret::parse(b"24681".to_vec()).is_err());
        assert!(PinSecret::parse(b"2468100".to_vec()).is_err());
        assert!(PinSecret::parse(b"24x810".to_vec()).is_err());
        assert!(PinSecret::parse(b" 246810".to_vec()).is_err());
        assert!(PinSecret::parse(b"246810\n\n".to_vec()).is_err());
    }

    #[test]
    fn fixed_length_compare_accepts_only_exact_secret() {
        let secret = secret();
        assert!(secret.matches(b"246810"));
        assert!(!secret.matches(b"246811"));
        assert!(!secret.matches(b"24681"));
        assert!(!secret.matches(b"2468100"));
    }

    #[test]
    fn arguments_never_include_a_secret() {
        let args = PinGateArgs::parse("prompt,5,2").unwrap();
        assert_eq!(args.prompt, "prompt");
        assert_eq!(args.deadline, Duration::from_secs(5));
        assert_eq!(args.digit_timeout, Duration::from_secs(2));
    }

    #[tokio::test]
    async fn deadline_cancels_bridge_lifetime_token() {
        let lifetime = BridgeLifetime::new();
        let deadline_lifetime = lifetime.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(5)).await;
            deadline_lifetime.cancel();
        });
        tokio::time::timeout(Duration::from_millis(100), lifetime.cancelled())
            .await
            .expect("deadline did not cancel the bridge lifetime token");
        assert!(lifetime.is_cancelled());
    }
}
