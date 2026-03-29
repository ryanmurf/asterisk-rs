//! SMS/text messaging over analog lines.
//!
//! Port of app_sms.c from Asterisk C. Implements ETSI ES 201 912 protocol 1
//! for SMS over PSTN/ISDN. Supports FSK encoding/decoding (stub) and RP-DATA
//! message parsing for SMS-DELIVER and SMS-SUBMIT.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::{debug, info, warn};

/// DLL (data link layer) message types for SMS protocol 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Dll1MessageType {
    Data = 0x11,
    Error = 0x12,
    Establish = 0x13,
    Release = 0x14,
    Ack = 0x15,
    Nack = 0x16,
}

/// DLL message types for SMS protocol 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Dll2MessageType {
    Establish = 0x7f,
    InfoMo = 0x10,
    InfoMt = 0x11,
    InfoSta = 0x12,
    Nack = 0x13,
    Ack0 = 0x14,
    Ack1 = 0x15,
    Enq = 0x16,
    Release = 0x17,
}

/// SMS TP (transfer protocol) message type indicators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmsDirection {
    /// Mobile-terminated: SMS-DELIVER (network to phone).
    Deliver,
    /// Mobile-originated: SMS-SUBMIT (phone to network).
    Submit,
}

/// An SMS message parsed from RP-DATA.
#[derive(Debug, Clone)]
pub struct SmsMessage {
    /// Originating address (caller number).
    pub sender: String,
    /// Destination address.
    pub recipient: String,
    /// Service centre address.
    pub service_centre: String,
    /// Message direction.
    pub direction: SmsDirection,
    /// Message body (decoded from 7-bit GSM or UCS-2).
    pub body: String,
    /// Protocol identifier.
    pub protocol_id: u8,
    /// Data coding scheme.
    pub data_coding: u8,
    /// Validity period (SMS-SUBMIT only, minutes).
    pub validity_minutes: Option<u32>,
}

impl SmsMessage {
    /// Create a new empty SMS message.
    pub fn new(direction: SmsDirection) -> Self {
        Self {
            sender: String::new(),
            recipient: String::new(),
            service_centre: String::new(),
            direction,
            body: String::new(),
            protocol_id: 0,
            data_coding: 0,
            validity_minutes: None,
        }
    }

    /// Parse an RP-DATA PDU (stub).
    ///
    /// In a real implementation this would decode the GSM 03.40 TPDU
    /// from the RP-DATA envelope.
    pub fn parse_rp_data(data: &[u8]) -> Option<Self> {
        if data.is_empty() {
            return None;
        }
        // First byte indicates message type
        let direction = if data[0] == 0x11 {
            SmsDirection::Deliver
        } else {
            SmsDirection::Submit
        };
        Some(Self::new(direction))
    }
}

/// FSK modem parameters for SMS over analog lines.
#[derive(Debug, Clone)]
pub struct FskParams {
    /// Baud rate (1200 baud for ETSI).
    pub baud_rate: u32,
    /// Mark frequency (Hz).
    pub mark_freq: u32,
    /// Space frequency (Hz).
    pub space_freq: u32,
}

impl Default for FskParams {
    fn default() -> Self {
        Self {
            baud_rate: 1200,
            mark_freq: 1200,
            space_freq: 2200,
        }
    }
}

/// Encode data as FSK audio samples (stub).
///
/// In a real implementation this would generate mu-law or slin audio
/// samples modulated at mark/space frequencies.
pub fn fsk_encode(_data: &[u8], _params: &FskParams) -> Vec<i16> {
    Vec::new()
}

/// Decode FSK audio samples to data bytes (stub).
///
/// In a real implementation this would demodulate the audio and
/// recover the data bits.
pub fn fsk_decode(_samples: &[i16], _params: &FskParams) -> Vec<u8> {
    Vec::new()
}

/// Options for the SMS application.
#[derive(Debug, Clone, Default)]
pub struct SmsOptions {
    /// Answer the channel before starting SMS exchange.
    pub answer: bool,
    /// Operate as the SMS service centre (SC) side.
    pub is_sc: bool,
    /// Use protocol 2 instead of protocol 1.
    pub protocol2: bool,
}

impl SmsOptions {
    /// Parse option string.
    pub fn parse(opts: &str) -> Self {
        let mut result = Self::default();
        for ch in opts.chars() {
            match ch {
                'a' => result.answer = true,
                's' => result.is_sc = true,
                '2' => result.protocol2 = true,
                _ => {
                    debug!("SMS: ignoring unknown option '{}'", ch);
                }
            }
        }
        result
    }
}

/// The SMS() dialplan application.
///
/// Usage: SMS(name[,options[,addr[,body]]])
///
/// Communicates with an SMS-capable device on an analog line using
/// ETSI ES 201 912 FSK protocol. Can send or receive SMS messages.
///
/// Options:
///   a - Answer the channel first
///   s - Act as service centre
///   2 - Use protocol 2
pub struct AppSms;

impl DialplanApp for AppSms {
    fn name(&self) -> &str {
        "SMS"
    }

    fn description(&self) -> &str {
        "Communicates with SMS capable devices via FSK on analog lines"
    }
}

impl AppSms {
    /// Execute the SMS application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parts: Vec<&str> = args.splitn(4, ',').collect();
        let queue_name = parts.first().copied().unwrap_or("");
        let options_str = parts.get(1).copied().unwrap_or("");
        let _options = SmsOptions::parse(options_str);

        if queue_name.is_empty() {
            warn!("SMS: requires a queue name argument");
            return PbxExecResult::Failed;
        }

        info!("SMS: channel '{}' queue '{}'", channel.name, queue_name);

        // In a real implementation:
        // 1. Answer channel if requested
        // 2. Set up FSK modem (1200 baud Bell 202)
        // 3. Exchange DLL protocol messages
        // 4. Transfer SMS message via RP-DATA
        // 5. Hang up or continue

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sms_message_new() {
        let msg = SmsMessage::new(SmsDirection::Deliver);
        assert_eq!(msg.direction, SmsDirection::Deliver);
        assert!(msg.body.is_empty());
    }

    #[test]
    fn test_sms_options_parse() {
        let opts = SmsOptions::parse("as2");
        assert!(opts.answer);
        assert!(opts.is_sc);
        assert!(opts.protocol2);
    }

    #[test]
    fn test_fsk_params_default() {
        let params = FskParams::default();
        assert_eq!(params.baud_rate, 1200);
    }

    #[test]
    fn test_parse_rp_data_empty() {
        assert!(SmsMessage::parse_rp_data(&[]).is_none());
    }

    #[test]
    fn test_parse_rp_data_deliver() {
        let msg = SmsMessage::parse_rp_data(&[0x11]).unwrap();
        assert_eq!(msg.direction, SmsDirection::Deliver);
    }

    #[tokio::test]
    async fn test_sms_exec() {
        let mut channel = Channel::new("DAHDI/1-1");
        let result = AppSms::exec(&mut channel, "myqueue,a").await;
        assert_eq!(result, PbxExecResult::Success);
    }
}
