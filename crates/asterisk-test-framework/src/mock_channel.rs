//! Mock Channel Technology for testing.
//!
//! Provides a mock implementation of `ChannelDriver` that records all operations
//! and allows configurable responses. Essential for testing bridges, applications,
//! and any code that interacts with channels.

use asterisk_core::channel::{Channel, ChannelDriver};
use asterisk_types::{AsteriskError, AsteriskResult, ControlFrame, Frame};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;

/// Records the state of all operations performed on a mock channel.
#[derive(Debug, Clone, Default)]
pub struct MockChannelState {
    /// All indication (control frame) requests received.
    pub indications: Vec<ControlFrame>,
    /// All frames written to the channel.
    pub written_frames: Vec<Frame>,
    /// Whether hangup was called.
    pub hungup: bool,
    /// Whether answer was called.
    pub answered: bool,
    /// All calls made (dest, timeout).
    pub calls: Vec<(String, i32)>,
    /// All DTMF digits sent via send_digit_begin.
    pub dtmf_begin_digits: Vec<char>,
    /// All DTMF digits sent via send_digit_end with durations.
    pub dtmf_end_digits: Vec<(char, u32)>,
    /// All text messages sent.
    pub sent_texts: Vec<String>,
}

/// Configurable responses for the mock channel.
#[derive(Debug, Default)]
pub struct MockChannelConfig {
    /// Frames queued to be returned by read_frame, in order.
    pub read_queue: VecDeque<Frame>,
    /// Whether to fail the next call attempt.
    pub fail_call: bool,
    /// Whether to fail the next answer attempt.
    pub fail_answer: bool,
    /// Whether to fail write_frame.
    pub fail_write: bool,
    /// Set of indications that are "accepted" (return Ok).
    /// If empty, all indications are accepted.
    pub accepted_indications: Vec<ControlFrame>,
}

impl MockChannelConfig {
    /// Queue a frame to be returned by the next read_frame call.
    pub fn queue_read_frame(&mut self, frame: Frame) {
        self.read_queue.push_back(frame);
    }

    /// Queue multiple frames.
    pub fn queue_read_frames(&mut self, frames: impl IntoIterator<Item = Frame>) {
        for f in frames {
            self.read_queue.push_back(f);
        }
    }
}

/// A mock channel technology that records operations and returns configured responses.
///
/// Thread-safe: state and config are behind `Mutex`.
pub struct MockChannelTech {
    /// Recorded operations.
    pub state: Arc<Mutex<MockChannelState>>,
    /// Configurable responses.
    pub config: Arc<Mutex<MockChannelConfig>>,
    /// Technology name.
    tech_name: String,
}

impl MockChannelTech {
    /// Create a new mock channel technology with default configuration.
    pub fn new() -> Self {
        Self::with_name("MockChannel")
    }

    /// Create a new mock channel technology with a specific name.
    pub fn with_name(name: &str) -> Self {
        MockChannelTech {
            state: Arc::new(Mutex::new(MockChannelState::default())),
            config: Arc::new(Mutex::new(MockChannelConfig::default())),
            tech_name: name.to_string(),
        }
    }

    /// Get a snapshot of the current recorded state.
    pub fn get_state(&self) -> MockChannelState {
        self.state.lock().clone()
    }

    /// Reset all recorded state.
    pub fn reset_state(&self) {
        *self.state.lock() = MockChannelState::default();
    }

    /// Get mutable access to the configuration.
    pub fn configure<F>(&self, f: F)
    where
        F: FnOnce(&mut MockChannelConfig),
    {
        let mut config = self.config.lock();
        f(&mut config);
    }

    /// Create a channel using this mock technology.
    pub fn create_channel(&self, name: &str) -> Channel {
        Channel::new(name)
    }
}

impl Default for MockChannelTech {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for MockChannelTech {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MockChannelTech")
            .field("name", &self.tech_name)
            .finish()
    }
}

#[async_trait::async_trait]
impl ChannelDriver for MockChannelTech {
    fn name(&self) -> &str {
        &self.tech_name
    }

    fn description(&self) -> &str {
        "Mock channel technology for testing"
    }

    async fn request(
        &self,
        dest: &str,
        _caller: Option<&Channel>,
    ) -> AsteriskResult<Channel> {
        let chan_name = format!("{}/{}", self.tech_name, dest);
        Ok(Channel::new(chan_name))
    }

    async fn call(
        &self,
        _channel: &mut Channel,
        dest: &str,
        timeout: i32,
    ) -> AsteriskResult<()> {
        let mut state = self.state.lock();
        state.calls.push((dest.to_string(), timeout));

        if self.config.lock().fail_call {
            return Err(AsteriskError::Internal("Mock call failure".into()));
        }
        Ok(())
    }

    async fn hangup(&self, _channel: &mut Channel) -> AsteriskResult<()> {
        self.state.lock().hungup = true;
        Ok(())
    }

    async fn answer(&self, _channel: &mut Channel) -> AsteriskResult<()> {
        if self.config.lock().fail_answer {
            return Err(AsteriskError::Internal("Mock answer failure".into()));
        }
        self.state.lock().answered = true;
        Ok(())
    }

    async fn read_frame(&self, _channel: &mut Channel) -> AsteriskResult<Frame> {
        let mut config = self.config.lock();
        if let Some(frame) = config.read_queue.pop_front() {
            Ok(frame)
        } else {
            // No more frames -- return a null frame (like a timing source)
            Ok(Frame::null())
        }
    }

    async fn write_frame(&self, _channel: &mut Channel, frame: &Frame) -> AsteriskResult<()> {
        if self.config.lock().fail_write {
            return Err(AsteriskError::Internal("Mock write failure".into()));
        }
        self.state.lock().written_frames.push(frame.clone());
        Ok(())
    }

    async fn indicate(
        &self,
        _channel: &mut Channel,
        condition: i32,
        _data: &[u8],
    ) -> AsteriskResult<()> {
        // Map the condition integer to ControlFrame
        // For our mock, we store a few known values
        let control = match condition {
            1 => ControlFrame::Hangup,
            2 => ControlFrame::Ring,
            3 => ControlFrame::Ringing,
            4 => ControlFrame::Answer,
            5 => ControlFrame::Busy,
            8 => ControlFrame::Congestion,
            14 => ControlFrame::Progress,
            15 => ControlFrame::Proceeding,
            16 => ControlFrame::Hold,
            17 => ControlFrame::Unhold,
            _ => {
                // Unknown condition -- still record it using a placeholder
                ControlFrame::Hangup // fallback
            }
        };

        let config = self.config.lock();
        if !config.accepted_indications.is_empty()
            && !config.accepted_indications.contains(&control)
        {
            return Err(AsteriskError::NotSupported(format!(
                "Indication {} not accepted by mock",
                condition
            )));
        }
        drop(config);

        self.state.lock().indications.push(control);
        Ok(())
    }

    async fn send_digit_begin(&self, _channel: &mut Channel, digit: char) -> AsteriskResult<()> {
        self.state.lock().dtmf_begin_digits.push(digit);
        Ok(())
    }

    async fn send_digit_end(
        &self,
        _channel: &mut Channel,
        digit: char,
        duration: u32,
    ) -> AsteriskResult<()> {
        self.state.lock().dtmf_end_digits.push((digit, duration));
        Ok(())
    }

    async fn send_text(&self, _channel: &mut Channel, text: &str) -> AsteriskResult<()> {
        self.state.lock().sent_texts.push(text.to_string());
        Ok(())
    }
}

#[cfg(test)]
mod mock_tests {
    use super::*;
    use bytes::Bytes;

    #[tokio::test]
    async fn test_mock_channel_request() {
        let mock = MockChannelTech::new();
        let chan = mock.request("alice", None).await.unwrap();
        assert_eq!(chan.name, "MockChannel/alice");
    }

    #[tokio::test]
    async fn test_mock_channel_call_record() {
        let mock = MockChannelTech::new();
        let mut chan = mock.create_channel("Test/001");
        mock.call(&mut chan, "bob", 30).await.unwrap();

        let state = mock.get_state();
        assert_eq!(state.calls.len(), 1);
        assert_eq!(state.calls[0].0, "bob");
        assert_eq!(state.calls[0].1, 30);
    }

    #[tokio::test]
    async fn test_mock_channel_read_queue() {
        let mock = MockChannelTech::new();
        let mut chan = mock.create_channel("Test/002");

        // Queue some frames
        mock.configure(|c| {
            c.queue_read_frame(Frame::voice(0, 160, Bytes::from_static(&[0x80; 160])));
            c.queue_read_frame(Frame::dtmf_begin('5'));
            c.queue_read_frame(Frame::control(ControlFrame::Hangup));
        });

        let f1 = mock.read_frame(&mut chan).await.unwrap();
        assert!(f1.is_voice());

        let f2 = mock.read_frame(&mut chan).await.unwrap();
        assert!(f2.is_dtmf());

        let f3 = mock.read_frame(&mut chan).await.unwrap();
        assert!(f3.is_control());

        // Queue is now empty, should return null frame
        let f4 = mock.read_frame(&mut chan).await.unwrap();
        assert_eq!(f4.frame_type(), asterisk_types::FrameType::Null);
    }

    #[tokio::test]
    async fn test_mock_channel_write_record() {
        let mock = MockChannelTech::new();
        let mut chan = mock.create_channel("Test/003");

        let frame = Frame::voice(0, 160, Bytes::from_static(&[0xFF; 160]));
        mock.write_frame(&mut chan, &frame).await.unwrap();

        let state = mock.get_state();
        assert_eq!(state.written_frames.len(), 1);
        assert!(state.written_frames[0].is_voice());
    }

    #[tokio::test]
    async fn test_mock_channel_hangup_answer() {
        let mock = MockChannelTech::new();
        let mut chan = mock.create_channel("Test/004");

        mock.answer(&mut chan).await.unwrap();
        assert!(mock.get_state().answered);

        mock.hangup(&mut chan).await.unwrap();
        assert!(mock.get_state().hungup);
    }

    #[tokio::test]
    async fn test_mock_channel_fail_call() {
        let mock = MockChannelTech::new();
        let mut chan = mock.create_channel("Test/005");
        mock.configure(|c| c.fail_call = true);

        let result = mock.call(&mut chan, "dest", 30).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_mock_channel_indicate() {
        let mock = MockChannelTech::new();
        let mut chan = mock.create_channel("Test/006");

        mock.indicate(&mut chan, 3, &[]).await.unwrap(); // Ringing
        mock.indicate(&mut chan, 4, &[]).await.unwrap(); // Answer

        let state = mock.get_state();
        assert_eq!(state.indications.len(), 2);
        assert_eq!(state.indications[0], ControlFrame::Ringing);
        assert_eq!(state.indications[1], ControlFrame::Answer);
    }

    #[tokio::test]
    async fn test_mock_channel_dtmf() {
        let mock = MockChannelTech::new();
        let mut chan = mock.create_channel("Test/007");

        mock.send_digit_begin(&mut chan, '1').await.unwrap();
        mock.send_digit_end(&mut chan, '1', 100).await.unwrap();

        let state = mock.get_state();
        assert_eq!(state.dtmf_begin_digits, vec!['1']);
        assert_eq!(state.dtmf_end_digits, vec![('1', 100)]);
    }

    #[tokio::test]
    async fn test_mock_channel_send_text() {
        let mock = MockChannelTech::new();
        let mut chan = mock.create_channel("Test/008");

        mock.send_text(&mut chan, "Hello World").await.unwrap();
        let state = mock.get_state();
        assert_eq!(state.sent_texts, vec!["Hello World"]);
    }

    #[tokio::test]
    async fn test_mock_channel_state_reset() {
        let mock = MockChannelTech::new();
        let mut chan = mock.create_channel("Test/009");

        mock.answer(&mut chan).await.unwrap();
        assert!(mock.get_state().answered);

        mock.reset_state();
        assert!(!mock.get_state().answered);
    }
}
