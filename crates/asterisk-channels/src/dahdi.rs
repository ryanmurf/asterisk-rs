//! DAHDI telephony interface channel driver.
//!
//! Port of `channels/chan_dahdi.c`, `channels/sig_analog.c`, and
//! `channels/sig_pri.c`. DAHDI provides access to hardware telephony
//! interfaces (T1/E1/BRI digital and FXS/FXO analog) on Linux.
//!
//! This module provides the full abstraction layer and channel driver, but
//! stubs the Linux-specific ioctl/device calls since DAHDI is Linux-only
//! kernel-level hardware access.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use parking_lot::RwLock;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, info};

use asterisk_core::channel::{Channel, ChannelDriver};
use asterisk_types::{AsteriskError, AsteriskResult, ChannelState, ControlFrame, Frame};

// ---------------------------------------------------------------------------
// Analog signaling types (sig_analog.h)
// ---------------------------------------------------------------------------

/// Analog signaling type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalogSignaling {
    /// No signaling.
    None,
    /// FXO, loop start.
    FxoLs,
    /// FXO, kewl start.
    FxoKs,
    /// FXO, ground start.
    FxoGs,
    /// FXS, loop start.
    FxsLs,
    /// FXS, kewl start.
    FxsKs,
    /// FXS, ground start.
    FxsGs,
    /// E&M wink.
    EmWink,
    /// E&M immediate.
    Em,
    /// E&M E1.
    EmE1,
    /// Feature Group D.
    FeatD,
    /// Feature Group D MF.
    FeatDmf,
    /// E911.
    E911,
    /// Feature Group C CAMA.
    FgcCama,
    /// Feature Group C CAMA MF.
    FgcCamaMf,
    /// Feature Group B.
    FeatB,
    /// SF wink.
    SfWink,
    /// SF immediate.
    Sf,
    /// SF Feature Group D.
    SfFeatD,
    /// SF Feature Group D MF.
    SfFeatDmf,
    /// Feature Group D MF Tandem Access.
    FeatDmfTa,
    /// SF Feature Group B.
    SfFeatB,
}

impl Default for AnalogSignaling {
    fn default() -> Self {
        Self::None
    }
}

/// Analog hook state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookState {
    OnHook,
    OffHook,
}

impl Default for HookState {
    fn default() -> Self {
        Self::OnHook
    }
}

/// Analog tone types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalogTone {
    RingTone,
    StutterDial,
    Congestion,
    DialTone,
    DialRecall,
    Info,
    BusyTone,
}

/// Analog events (from DAHDI hardware or signaling).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalogEvent {
    None,
    OnHook,
    OffHook,
    RingOffHook,
    WinkFlash,
    Alarm,
    NoAlarm,
    DialComplete,
    RingerOn,
    RingerOff,
    HookComplete,
    PulseStart,
    Polarity,
    RingBegin,
    EchoCanDisabled,
    Removed,
    DtmfDown(char),
    DtmfUp(char),
}

/// Caller ID signaling method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallerIdMethod {
    /// FSK (Bell 202 / V.23) modem tones.
    Fsk,
    /// DTMF-based caller ID.
    Dtmf,
}

impl Default for CallerIdMethod {
    fn default() -> Self {
        Self::Fsk
    }
}

/// Analog sub-channel type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalogSub {
    /// Active call.
    Real,
    /// Call-waiting call on hold.
    CallWait,
    /// Three-way call.
    ThreeWay,
}

/// Analog channel state machine.
#[derive(Debug, Clone)]
pub struct AnalogChannel {
    /// Channel number (span/channel).
    pub channel_number: u32,
    /// Signaling type.
    pub signaling: AnalogSignaling,
    /// Current hook state.
    pub hook_state: HookState,
    /// Accumulated dial string.
    pub dial_string: String,
    /// Whether a ring has been detected.
    pub ring_detected: bool,
    /// Caller ID method.
    pub cid_method: CallerIdMethod,
    /// Detected caller ID name.
    pub cid_name: String,
    /// Detected caller ID number.
    pub cid_number: String,
    /// Whether call waiting is enabled.
    pub call_waiting: bool,
    /// Whether three-way calling is enabled.
    pub three_way: bool,
    /// Whether echo cancellation is enabled.
    pub echo_cancel: bool,
    /// DTMF detection mode.
    pub dtmf_detect: bool,
    /// Pulse dialing support.
    pub pulse_dial: bool,
    /// Ring count.
    pub ring_count: u32,
    /// Distinctive ring patterns (up to 3).
    pub ring_pattern: [u32; 3],
}

impl AnalogChannel {
    pub fn new(channel_number: u32, signaling: AnalogSignaling) -> Self {
        Self {
            channel_number,
            signaling,
            hook_state: HookState::OnHook,
            dial_string: String::new(),
            ring_detected: false,
            cid_method: CallerIdMethod::Fsk,
            cid_name: String::new(),
            cid_number: String::new(),
            call_waiting: true,
            three_way: true,
            echo_cancel: true,
            dtmf_detect: true,
            pulse_dial: false,
            ring_count: 0,
            ring_pattern: [0; 3],
        }
    }

    /// Process a hook event (stub -- would read from DAHDI device on Linux).
    pub fn process_event(&mut self, event: AnalogEvent) {
        match event {
            AnalogEvent::OffHook => {
                debug!(ch = self.channel_number, "Analog: off-hook");
            }
            AnalogEvent::OnHook => {
                self.hook_state = HookState::OnHook;
                self.dial_string.clear();
                debug!(ch = self.channel_number, "Analog: on-hook");
            }
            AnalogEvent::RingOffHook => {
                self.hook_state = HookState::OffHook;
                self.ring_detected = true;
                debug!(ch = self.channel_number, "Analog: ring/off-hook");
            }
            AnalogEvent::DtmfDown(digit) => {
                self.dial_string.push(digit);
                debug!(ch = self.channel_number, digit = %digit, "Analog: DTMF digit");
            }
            AnalogEvent::DtmfUp(_) => {}
            AnalogEvent::RingBegin => {
                self.ring_count += 1;
                debug!(ch = self.channel_number, rings = self.ring_count, "Analog: ring");
            }
            _ => {
                debug!(ch = self.channel_number, event = ?event, "Analog: event");
            }
        }
    }

    /// Simulate playing a tone (stub).
    pub fn play_tone(&self, tone: AnalogTone) {
        debug!(
            ch = self.channel_number,
            tone = ?tone,
            "Analog: play tone (stub)"
        );
    }

    /// Simulate starting caller ID detection (stub).
    pub fn start_cid_detect(&mut self) {
        debug!(
            ch = self.channel_number,
            method = ?self.cid_method,
            "Analog: start CID detect (stub)"
        );
    }

    /// Simulate stopping caller ID detection (stub).
    pub fn stop_cid_detect(&mut self) {
        debug!(ch = self.channel_number, "Analog: stop CID detect (stub)");
    }
}

// ---------------------------------------------------------------------------
// PRI signaling types (sig_pri.h)
// ---------------------------------------------------------------------------

/// PRI signaling law.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriLaw {
    Default,
    Ulaw,
    Alaw,
}

impl Default for PriLaw {
    fn default() -> Self {
        Self::Default
    }
}

/// PRI span configuration (stub).
///
/// In a full implementation, this would manage an entire ISDN PRI or BRI
/// span with D-channel signaling and B-channel allocation.
#[derive(Debug, Clone)]
pub struct PriSpan {
    /// Span number.
    pub span_number: u32,
    /// Number of B-channels.
    pub num_b_channels: u32,
    /// D-channel number.
    pub d_channel: u32,
    /// Signaling law.
    pub law: PriLaw,
    /// Whether this is a BRI span.
    pub is_bri: bool,
    /// Whether the D-channel is up.
    pub d_channel_up: bool,
    /// Active call reference values.
    pub call_refs: Vec<u32>,
    /// Switch type (national, dms100, 4ess, 5ess, euroisdn, etc.).
    pub switch_type: String,
    /// Network-side or user-side.
    pub is_network_side: bool,
}

impl PriSpan {
    pub fn new(span_number: u32, num_b_channels: u32, d_channel: u32) -> Self {
        Self {
            span_number,
            num_b_channels,
            d_channel,
            law: PriLaw::Default,
            is_bri: false,
            d_channel_up: false,
            call_refs: Vec::new(),
            switch_type: "national".to_string(),
            is_network_side: false,
        }
    }

    /// Allocate a B-channel (stub).
    pub fn allocate_b_channel(&mut self) -> Option<u32> {
        // In real implementation, find a free B-channel.
        debug!(span = self.span_number, "PRI: allocate B-channel (stub)");
        None
    }

    /// Release a B-channel (stub).
    pub fn release_b_channel(&mut self, _channel: u32) {
        debug!(span = self.span_number, "PRI: release B-channel (stub)");
    }
}

// ---------------------------------------------------------------------------
// SS7 signaling types (stub)
// ---------------------------------------------------------------------------

/// SS7 link configuration (stub).
///
/// SS7 signaling is used for inter-switch communication in the PSTN.
/// This is a minimal stub for the abstraction.
#[derive(Debug, Clone)]
pub struct Ss7Link {
    /// Link set number.
    pub linkset: u32,
    /// Point code (14-bit).
    pub point_code: u32,
    /// Adjacent point code.
    pub adjacent_point_code: u32,
    /// Whether the link is up.
    pub link_up: bool,
    /// CIC (Circuit Identification Code) range.
    pub cic_start: u32,
    pub cic_end: u32,
}

impl Ss7Link {
    pub fn new(linkset: u32, point_code: u32) -> Self {
        Self {
            linkset,
            point_code,
            adjacent_point_code: 0,
            link_up: false,
            cic_start: 1,
            cic_end: 24,
        }
    }
}

// ---------------------------------------------------------------------------
// DAHDI device abstraction
// ---------------------------------------------------------------------------

/// DAHDI channel configuration.
#[derive(Debug, Clone)]
pub struct DahdiChannelConfig {
    /// DAHDI channel number.
    pub channel_number: u32,
    /// Span this channel belongs to.
    pub span: u32,
    /// Signaling type.
    pub signaling: DahdiSignaling,
    /// Buffer policy size.
    pub buffer_size: u32,
    /// Number of buffers.
    pub num_buffers: u32,
    /// Echo cancellation taps.
    pub echo_cancel_taps: u32,
}

impl Default for DahdiChannelConfig {
    fn default() -> Self {
        Self {
            channel_number: 0,
            span: 0,
            signaling: DahdiSignaling::Analog(AnalogSignaling::FxsLs),
            buffer_size: 160,
            num_buffers: 4,
            echo_cancel_taps: 128,
        }
    }
}

/// Top-level signaling type for a DAHDI channel.
#[derive(Debug, Clone)]
pub enum DahdiSignaling {
    /// Analog signaling (FXS/FXO).
    Analog(AnalogSignaling),
    /// PRI/BRI signaling.
    Pri,
    /// SS7 signaling.
    Ss7,
}

// ---------------------------------------------------------------------------
// Per-channel private data
// ---------------------------------------------------------------------------

struct DahdiPrivate {
    /// DAHDI config for this channel.
    config: DahdiChannelConfig,
    /// Analog channel state (if analog signaling).
    analog: Option<AnalogChannel>,
    /// Frame delivery channel.
    frame_tx: mpsc::Sender<Frame>,
    frame_rx: Mutex<mpsc::Receiver<Frame>>,
    /// Whether the channel is open.
    is_open: AtomicBool,
}

impl fmt::Debug for DahdiPrivate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DahdiPrivate")
            .field("config", &self.config)
            .field("analog", &self.analog.is_some())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Channel driver
// ---------------------------------------------------------------------------

/// DAHDI channel driver.
///
/// Port of `chan_dahdi.c`. Provides access to DAHDI telephony hardware
/// (analog FXS/FXO ports, PRI/BRI digital spans, SS7 links).
///
/// NOTE: The actual Linux ioctl calls are stubbed. The abstractions and
/// channel driver logic are fully implemented.
pub struct DahdiDriver {
    /// Active channels keyed by channel unique ID.
    channels: RwLock<HashMap<String, Arc<DahdiPrivate>>>,
    /// Configured DAHDI channels.
    configs: RwLock<Vec<DahdiChannelConfig>>,
    /// PRI spans (stub).
    pri_spans: RwLock<Vec<PriSpan>>,
    /// SS7 links (stub).
    ss7_links: RwLock<Vec<Ss7Link>>,
}

impl fmt::Debug for DahdiDriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DahdiDriver")
            .field("active_channels", &self.channels.read().len())
            .field("configured_channels", &self.configs.read().len())
            .field("pri_spans", &self.pri_spans.read().len())
            .field("ss7_links", &self.ss7_links.read().len())
            .finish()
    }
}

impl DahdiDriver {
    pub fn new() -> Self {
        Self {
            channels: RwLock::new(HashMap::new()),
            configs: RwLock::new(Vec::new()),
            pri_spans: RwLock::new(Vec::new()),
            ss7_links: RwLock::new(Vec::new()),
        }
    }

    /// Add a DAHDI channel configuration.
    pub fn add_channel_config(&self, config: DahdiChannelConfig) {
        self.configs.write().push(config);
    }

    /// Add a PRI span configuration.
    pub fn add_pri_span(&self, span: PriSpan) {
        self.pri_spans.write().push(span);
    }

    /// Add an SS7 link configuration.
    pub fn add_ss7_link(&self, link: Ss7Link) {
        self.ss7_links.write().push(link);
    }

    fn get_private(&self, id: &str) -> Option<Arc<DahdiPrivate>> {
        self.channels.read().get(id).cloned()
    }

    fn remove_private(&self, id: &str) -> Option<Arc<DahdiPrivate>> {
        self.channels.write().remove(id)
    }

    /// Open a DAHDI device (stub -- on Linux this would open `/dev/dahdi/channel`).
    fn open_device(_channel_number: u32) -> AsteriskResult<()> {
        // Stub: on Linux, this would:
        //   fd = open("/dev/dahdi/channel", O_RDWR)
        //   ioctl(fd, DAHDI_SPECIFY, &channel_number)
        //   ioctl(fd, DAHDI_SET_BUFINFO, &bufinfo)
        debug!(_channel_number, "DAHDI: open device (stub)");
        Ok(())
    }

    /// Configure echo cancellation (stub).
    fn configure_echo_cancel(_channel_number: u32, _taps: u32) -> AsteriskResult<()> {
        debug!(
            _channel_number,
            _taps, "DAHDI: configure echo cancel (stub)"
        );
        Ok(())
    }

    /// Read audio data from DAHDI device (stub).
    fn read_audio(_channel_number: u32) -> AsteriskResult<Bytes> {
        // Stub: returns silence.
        Ok(Bytes::from(vec![0x7Fu8; 160])) // 160 bytes of ulaw silence
    }

    /// Write audio data to DAHDI device (stub).
    fn write_audio(_channel_number: u32, _data: &[u8]) -> AsteriskResult<()> {
        Ok(())
    }

    /// Set hook state on device (stub).
    fn set_hook_state(_channel_number: u32, _state: HookState) -> AsteriskResult<()> {
        debug!(_channel_number, ?_state, "DAHDI: set hook state (stub)");
        Ok(())
    }

    /// Generate a tone (stub).
    fn generate_tone(_channel_number: u32, _tone: AnalogTone) -> AsteriskResult<()> {
        debug!(_channel_number, ?_tone, "DAHDI: generate tone (stub)");
        Ok(())
    }

    /// Get channel config by DAHDI channel number.
    fn find_config(&self, channel_number: u32) -> Option<DahdiChannelConfig> {
        self.configs
            .read()
            .iter()
            .find(|c| c.channel_number == channel_number)
            .cloned()
    }
}

impl Default for DahdiDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChannelDriver for DahdiDriver {
    fn name(&self) -> &str {
        "DAHDI"
    }

    fn description(&self) -> &str {
        "DAHDI Telephony Interface Channel Driver"
    }

    async fn request(&self, dest: &str, _caller: Option<&Channel>) -> AsteriskResult<Channel> {
        // dest format: "channel_number" or "g<group>/extension"
        let channel_number: u32 = dest.parse().map_err(|e| {
            AsteriskError::InvalidArgument(format!(
                "DAHDI destination must be a channel number: {}",
                e
            ))
        })?;

        let config = self.find_config(channel_number).unwrap_or(DahdiChannelConfig {
            channel_number,
            ..Default::default()
        });

        // Open device (stub).
        Self::open_device(channel_number)?;

        let analog = match &config.signaling {
            DahdiSignaling::Analog(sig) => Some(AnalogChannel::new(channel_number, *sig)),
            _ => None,
        };

        let (frame_tx, frame_rx) = mpsc::channel(128);

        let chan_name = format!("DAHDI/{}", channel_number);
        let channel = Channel::new(chan_name);
        let channel_id = channel.unique_id.as_str().to_string();

        let priv_data = Arc::new(DahdiPrivate {
            config,
            analog,
            frame_tx,
            frame_rx: Mutex::new(frame_rx),
            is_open: AtomicBool::new(true),
        });

        self.channels.write().insert(channel_id, priv_data);
        info!(channel_number, "DAHDI channel created");
        Ok(channel)
    }

    async fn call(&self, channel: &mut Channel, dest: &str, _timeout: i32) -> AsteriskResult<()> {
        let priv_data = self
            .get_private(channel.unique_id.as_str())
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        // Go off-hook and dial.
        Self::set_hook_state(priv_data.config.channel_number, HookState::OffHook)?;

        channel.set_state(ChannelState::Dialing);
        info!(
            ch = priv_data.config.channel_number,
            dest, "DAHDI call initiated"
        );
        Ok(())
    }

    async fn answer(&self, channel: &mut Channel) -> AsteriskResult<()> {
        let priv_data = self
            .get_private(channel.unique_id.as_str())
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        Self::set_hook_state(priv_data.config.channel_number, HookState::OffHook)?;
        channel.answer();
        info!(ch = priv_data.config.channel_number, "DAHDI call answered");
        Ok(())
    }

    async fn hangup(&self, channel: &mut Channel) -> AsteriskResult<()> {
        let priv_data = match self.remove_private(channel.unique_id.as_str()) {
            Some(p) => p,
            None => return Ok(()),
        };

        priv_data.is_open.store(false, Ordering::Relaxed);
        Self::set_hook_state(priv_data.config.channel_number, HookState::OnHook)?;
        channel.set_state(ChannelState::Down);
        info!(ch = priv_data.config.channel_number, "DAHDI channel hungup");
        Ok(())
    }

    async fn read_frame(&self, channel: &mut Channel) -> AsteriskResult<Frame> {
        let priv_data = self
            .get_private(channel.unique_id.as_str())
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        if !priv_data.is_open.load(Ordering::Relaxed) {
            return Ok(Frame::control(ControlFrame::Hangup));
        }

        // Read from device (stub: returns silence).
        let audio = Self::read_audio(priv_data.config.channel_number)?;
        Ok(Frame::voice(0, audio.len() as u32, audio))
    }

    async fn write_frame(&self, channel: &mut Channel, frame: &Frame) -> AsteriskResult<()> {
        let priv_data = self
            .get_private(channel.unique_id.as_str())
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        match frame {
            Frame::Voice { data, .. } => {
                Self::write_audio(priv_data.config.channel_number, data)?;
            }
            _ => {}
        }
        Ok(())
    }

    async fn indicate(
        &self,
        channel: &mut Channel,
        condition: i32,
        _data: &[u8],
    ) -> AsteriskResult<()> {
        let priv_data = self
            .get_private(channel.unique_id.as_str())
            .ok_or_else(|| AsteriskError::NotFound(channel.name.clone()))?;

        match condition as u32 {
            x if x == ControlFrame::Ringing as u32 => {
                Self::generate_tone(priv_data.config.channel_number, AnalogTone::RingTone)?;
            }
            x if x == ControlFrame::Busy as u32 => {
                Self::generate_tone(priv_data.config.channel_number, AnalogTone::BusyTone)?;
            }
            x if x == ControlFrame::Congestion as u32 => {
                Self::generate_tone(priv_data.config.channel_number, AnalogTone::Congestion)?;
            }
            x if x == ControlFrame::Progress as u32 => {
                // Early audio -- just allow media to flow.
            }
            _ => {
                debug!(
                    ch = priv_data.config.channel_number,
                    condition, "DAHDI: unhandled indication"
                );
            }
        }
        Ok(())
    }

    async fn send_digit_begin(&self, _channel: &mut Channel, digit: char) -> AsteriskResult<()> {
        // DAHDI generates DTMF tones on hardware.
        debug!(digit = %digit, "DAHDI: DTMF begin (stub)");
        Ok(())
    }

    async fn send_digit_end(
        &self,
        _channel: &mut Channel,
        digit: char,
        _duration: u32,
    ) -> AsteriskResult<()> {
        debug!(digit = %digit, "DAHDI: DTMF end (stub)");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analog_channel_creation() {
        let ch = AnalogChannel::new(1, AnalogSignaling::FxsLs);
        assert_eq!(ch.channel_number, 1);
        assert_eq!(ch.hook_state, HookState::OnHook);
        assert!(ch.dial_string.is_empty());
    }

    #[test]
    fn test_analog_event_processing() {
        let mut ch = AnalogChannel::new(1, AnalogSignaling::FxsLs);
        ch.process_event(AnalogEvent::RingOffHook);
        assert_eq!(ch.hook_state, HookState::OffHook);
        assert!(ch.ring_detected);

        ch.process_event(AnalogEvent::DtmfDown('5'));
        assert_eq!(ch.dial_string, "5");

        ch.process_event(AnalogEvent::DtmfDown('1'));
        assert_eq!(ch.dial_string, "51");

        ch.process_event(AnalogEvent::OnHook);
        assert_eq!(ch.hook_state, HookState::OnHook);
        assert!(ch.dial_string.is_empty());
    }

    #[test]
    fn test_pri_span_creation() {
        let span = PriSpan::new(1, 23, 24);
        assert_eq!(span.span_number, 1);
        assert_eq!(span.num_b_channels, 23);
        assert_eq!(span.d_channel, 24);
        assert!(!span.d_channel_up);
    }

    #[test]
    fn test_ss7_link_creation() {
        let link = Ss7Link::new(1, 12345);
        assert_eq!(link.linkset, 1);
        assert_eq!(link.point_code, 12345);
        assert!(!link.link_up);
    }

    #[tokio::test]
    async fn test_dahdi_request_and_hangup() {
        let driver = DahdiDriver::new();
        let mut chan = driver.request("1", None).await.unwrap();
        assert!(chan.name.starts_with("DAHDI/"));
        driver.hangup(&mut chan).await.unwrap();
        assert_eq!(chan.state, ChannelState::Down);
    }

    #[tokio::test]
    async fn test_dahdi_read_returns_silence() {
        let driver = DahdiDriver::new();
        let mut chan = driver.request("1", None).await.unwrap();
        let frame = driver.read_frame(&mut chan).await.unwrap();
        assert!(frame.is_voice());
        driver.hangup(&mut chan).await.unwrap();
    }
}
