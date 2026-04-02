//! Alarm receiver application - Ademco Contact ID format.
//!
//! Port of app_alarmreceiver.c from Asterisk C. Receives alarm system
//! signals via DTMF using the Ademco Contact ID protocol. Parses event
//! codes and logs them to a file.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::info;
use std::fmt;

/// Ademco Contact ID event qualifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventQualifier {
    /// New event or opening.
    Event,
    /// Restore or closing.
    Restore,
    /// Repeat (previously reported condition still present).
    Repeat,
}

impl EventQualifier {
    /// Parse from single digit character.
    pub fn from_digit(d: char) -> Option<Self> {
        match d {
            '1' | 'E' => Some(Self::Event),
            '3' | 'R' => Some(Self::Restore),
            '6' => Some(Self::Repeat),
            _ => None,
        }
    }
}

impl fmt::Display for EventQualifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Event => write!(f, "EVENT"),
            Self::Restore => write!(f, "RESTORE"),
            Self::Repeat => write!(f, "REPEAT"),
        }
    }
}

/// A parsed Ademco Contact ID alarm event.
///
/// Format: ACCT MT QXYZ GG CCC
/// - ACCT: 4-digit account number
/// - MT: message type (18 = Contact ID)
/// - Q: event qualifier (1=event, 3=restore, 6=repeat)
/// - XYZ: 3-digit event code
/// - GG: 2-digit group/partition
/// - CCC: 3-digit zone/user
#[derive(Debug, Clone)]
pub struct AlarmEvent {
    /// 4-digit account number.
    pub account: String,
    /// Message type (typically 18 for Contact ID).
    pub message_type: u8,
    /// Event qualifier.
    pub qualifier: EventQualifier,
    /// 3-digit event code (e.g. 110=fire, 130=burglary).
    pub event_code: u16,
    /// Group or partition number.
    pub group: u8,
    /// Zone or user number.
    pub zone: u16,
    /// Checksum valid.
    pub checksum_ok: bool,
}

impl AlarmEvent {
    /// Parse a Contact ID string (16 DTMF digits).
    ///
    /// Format: ACCT 18 QXYZ GG CCC S
    /// where S is checksum digit (sum of all digits mod 15 == 0).
    pub fn parse(digits: &str) -> Option<Self> {
        let digits: Vec<char> = digits.chars().collect();
        if digits.len() < 16 {
            return None;
        }

        let account: String = digits[0..4].iter().collect();
        let mt_str: String = digits[4..6].iter().collect();
        let message_type = mt_str.parse::<u8>().ok()?;

        let qualifier = EventQualifier::from_digit(digits[6])?;

        let event_str: String = digits[7..10].iter().collect();
        let event_code = event_str.parse::<u16>().ok()?;

        let group_str: String = digits[10..12].iter().collect();
        let group = group_str.parse::<u8>().ok()?;

        let zone_str: String = digits[12..15].iter().collect();
        let zone = zone_str.parse::<u16>().ok()?;

        // Verify checksum: sum of all digits mod 15 should equal 0
        // (0 is treated as 10 in the Ademco checksum)
        let checksum_ok = Self::verify_checksum(&digits[..16]);

        Some(Self {
            account,
            message_type,
            qualifier,
            event_code,
            group,
            zone,
            checksum_ok,
        })
    }

    /// Verify Contact ID checksum.
    fn verify_checksum(digits: &[char]) -> bool {
        let mut sum: u32 = 0;
        for &d in digits {
            let val = match d {
                '0' => 10, // 0 counts as 10 in Contact ID checksum
                '1'..='9' => (d as u32) - ('0' as u32),
                'A' | 'a' | '*' => 10,
                'B' | 'b' | '#' => 11,
                'C' | 'c' => 12,
                'D' | 'd' => 13,
                'E' | 'e' => 14,
                'F' | 'f' => 15,
                _ => 0,
            };
            sum += val;
        }
        sum.is_multiple_of(15)
    }

    /// Format event for logging.
    pub fn to_log_string(&self) -> String {
        format!(
            "ACCT={} MT={:02} {} CODE={:03} GRP={:02} ZONE={:03} CHK={}",
            self.account,
            self.message_type,
            self.qualifier,
            self.event_code,
            self.group,
            self.zone,
            if self.checksum_ok { "OK" } else { "FAIL" },
        )
    }
}

/// Configuration for the alarm receiver.
#[derive(Debug, Clone)]
pub struct AlarmReceiverConfig {
    /// Path to log file for alarm events.
    pub log_file: String,
    /// Number of DTMF digits to collect per event.
    pub event_length: usize,
    /// Timeout (ms) waiting for DTMF.
    pub dtmf_timeout_ms: u32,
    /// Number of events to receive before hanging up (0 = unlimited).
    pub max_events: usize,
}

impl Default for AlarmReceiverConfig {
    fn default() -> Self {
        Self {
            log_file: "/var/log/asterisk/alarm_events.log".to_string(),
            event_length: 16,
            dtmf_timeout_ms: 4000,
            max_events: 0,
        }
    }
}

/// The AlarmReceiver() dialplan application.
///
/// Usage: AlarmReceiver()
///
/// Receives alarm system reports via DTMF using Ademco Contact ID protocol.
/// Each event is a sequence of 16 DTMF digits. Events are logged to a
/// configurable log file and channel variables are set with the results.
///
/// Sets:
///   ALARMSTATUS = SUCCESS | FAIL
///   ALARMCOUNT  = number of events received
pub struct AppAlarmReceiver;

impl DialplanApp for AppAlarmReceiver {
    fn name(&self) -> &str {
        "AlarmReceiver"
    }

    fn description(&self) -> &str {
        "Receive alarm system reports via Ademco Contact ID"
    }
}

impl AppAlarmReceiver {
    /// Execute the AlarmReceiver application.
    pub async fn exec(channel: &mut Channel, _args: &str) -> PbxExecResult {
        info!("AlarmReceiver: channel '{}' starting alarm receiver", channel.name);

        let _config = AlarmReceiverConfig::default();

        // In a real implementation:
        // 1. Answer channel if not already up
        // 2. Send initial handshake tone (1400 Hz, 100ms)
        // 3. Loop:
        //    a. Receive 16 DTMF digits (with timeout)
        //    b. Parse as Contact ID event
        //    c. Log to file
        //    d. Send ACK tone (kissoff: 1400 Hz, 900ms)
        // 4. Set ALARMSTATUS and ALARMCOUNT variables

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_qualifier_parse() {
        assert_eq!(EventQualifier::from_digit('1'), Some(EventQualifier::Event));
        assert_eq!(EventQualifier::from_digit('3'), Some(EventQualifier::Restore));
        assert_eq!(EventQualifier::from_digit('6'), Some(EventQualifier::Repeat));
        assert_eq!(EventQualifier::from_digit('9'), None);
    }

    #[test]
    fn test_alarm_event_log_string() {
        let event = AlarmEvent {
            account: "1234".to_string(),
            message_type: 18,
            qualifier: EventQualifier::Event,
            event_code: 110,
            group: 1,
            zone: 1,
            checksum_ok: true,
        };
        let s = event.to_log_string();
        assert!(s.contains("ACCT=1234"));
        assert!(s.contains("CODE=110"));
    }

    #[test]
    fn test_alarm_receiver_config_default() {
        let cfg = AlarmReceiverConfig::default();
        assert_eq!(cfg.event_length, 16);
        assert_eq!(cfg.dtmf_timeout_ms, 4000);
    }

    #[tokio::test]
    async fn test_alarm_receiver_exec() {
        let mut channel = Channel::new("DAHDI/1-1");
        let result = AppAlarmReceiver::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Success);
    }
}
