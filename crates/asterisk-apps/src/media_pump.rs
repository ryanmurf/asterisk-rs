//! Two-way bridge media pumps with bounded RTP reordering.

use asterisk_core::bridge::lifetime::BridgeLifetime;
use asterisk_core::channel::{Channel, ChannelDriver};
use asterisk_types::{AsteriskResult, Frame, RtpTiming};
use std::sync::Arc;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

/// Maximum number of out-of-order voice frames retained per direction.
const REORDER_CAPACITY: usize = 4;

#[derive(Debug)]
struct VoiceReorderBuffer {
    expected: Option<u16>,
    pending: Vec<Frame>,
    capacity: usize,
}

impl VoiceReorderBuffer {
    fn new(capacity: usize) -> Self {
        assert!(capacity > 0);
        Self {
            expected: None,
            pending: Vec::with_capacity(capacity),
            capacity,
        }
    }

    fn timing(frame: &Frame) -> Option<RtpTiming> {
        match frame {
            Frame::Voice { rtp_timing, .. } => *rtp_timing,
            _ => None,
        }
    }

    /// Insert one voice frame and return every frame now safe to emit.
    /// Old/duplicate packets are discarded, and a persistent gap is released
    /// once the fixed-size window fills so memory and latency stay bounded.
    fn push(&mut self, frame: Frame) -> Vec<Frame> {
        let Some(timing) = Self::timing(&frame) else {
            return vec![frame];
        };

        let Some(expected) = self.expected else {
            self.expected = Some(timing.sequence.wrapping_add(1));
            return vec![frame];
        };

        let distance = timing.sequence.wrapping_sub(expected);
        if distance >= 0x8000 {
            return Vec::new();
        }
        if self.pending.iter().any(|pending| {
            Self::timing(pending).is_some_and(|value| value.sequence == timing.sequence)
        }) {
            return Vec::new();
        }

        self.pending.push(frame);
        if self.pending.len() == self.capacity && !self.contains_sequence(expected) {
            let next = self
                .pending
                .iter()
                .filter_map(Self::timing)
                .min_by_key(|value| value.sequence.wrapping_sub(expected))
                .expect("non-empty timed reorder buffer");
            self.expected = Some(next.sequence);
        }

        self.drain_contiguous()
    }

    fn contains_sequence(&self, sequence: u16) -> bool {
        self.pending.iter().any(|frame| {
            Self::timing(frame).is_some_and(|timing| timing.sequence == sequence)
        })
    }

    fn drain_contiguous(&mut self) -> Vec<Frame> {
        let mut ready = Vec::new();
        while let Some(expected) = self.expected {
            let Some(index) = self.pending.iter().position(|frame| {
                Self::timing(frame).is_some_and(|timing| timing.sequence == expected)
            }) else {
                break;
            };
            ready.push(self.pending.swap_remove(index));
            self.expected = Some(expected.wrapping_add(1));
        }
        ready
    }
}

/// The exactly two tasks owned by one two-party media bridge.
pub(crate) struct MediaPumps {
    first: JoinHandle<()>,
    second: JoinHandle<()>,
}

impl MediaPumps {
    pub(crate) async fn wait(self) {
        if let Err(error) = self.first.await {
            warn!(%error, "first bridge media pump task failed");
        }
        if let Err(error) = self.second.await {
            warn!(%error, "second bridge media pump task failed");
        }
    }
}

pub(crate) fn start_media_pumps(
    first_name: String,
    first_driver: Arc<dyn ChannelDriver>,
    second_name: String,
    second_driver: Arc<dyn ChannelDriver>,
    lifetime: BridgeLifetime,
) -> MediaPumps {
    let first = spawn_direction(
        first_name.clone(),
        first_driver.clone(),
        second_name.clone(),
        second_driver.clone(),
        lifetime.clone(),
    );
    let second = spawn_direction(
        second_name,
        second_driver,
        first_name,
        first_driver,
        lifetime,
    );
    MediaPumps { first, second }
}

fn spawn_direction(
    source_name: String,
    source_driver: Arc<dyn ChannelDriver>,
    destination_name: String,
    destination_driver: Arc<dyn ChannelDriver>,
    lifetime: BridgeLifetime,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let result = pump_direction(
            &source_name,
            source_driver,
            &destination_name,
            destination_driver,
            &lifetime,
        )
        .await;
        if let Err(error) = result {
            warn!(source = %source_name, destination = %destination_name, %error,
                "bridge media pump exited with an error");
        }
        // A read error, write error, or normal peer exit is a bridge-lifetime
        // event. It must wake the opposite task even when that RTP source is silent.
        lifetime.cancel();
        debug!(source = %source_name, destination = %destination_name,
            "bridge media pump stopped");
    })
}

async fn pump_direction(
    source_name: &str,
    source_driver: Arc<dyn ChannelDriver>,
    destination_name: &str,
    destination_driver: Arc<dyn ChannelDriver>,
    lifetime: &BridgeLifetime,
) -> AsteriskResult<()> {
    // SIP drivers route media by channel name. Independent handles prevent
    // either blocking read from holding the Dial-owned Channel value.
    let mut source = Channel::new(source_name);
    let mut destination = Channel::new(destination_name);
    let mut reorder = VoiceReorderBuffer::new(REORDER_CAPACITY);

    loop {
        let frame = tokio::select! {
            biased;
            _ = lifetime.cancelled() => return Ok(()),
            result = source_driver.read_frame(&mut source) => result?,
        };

        match frame {
            voice @ Frame::Voice { .. } => {
                for ready in reorder.push(voice) {
                    tokio::select! {
                        biased;
                        _ = lifetime.cancelled() => return Ok(()),
                        result = destination_driver.write_frame(&mut destination, &ready) => result?,
                    }
                }
            }
            Frame::DtmfEnd { digit, duration_ms } => {
                tokio::select! {
                    biased;
                    _ = lifetime.cancelled() => return Ok(()),
                    result = destination_driver.send_digit_end(
                        &mut destination,
                        digit,
                        duration_ms,
                    ) => result?,
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use asterisk_types::{AsteriskError, ChannelState};
    use bytes::Bytes;
    use parking_lot::Mutex as ParkingMutex;
    use std::time::Duration;
    use tokio::sync::{mpsc, Mutex};

    fn timed_voice(sequence: u16, timestamp: u32, value: u8) -> Frame {
        Frame::voice_with_rtp_timing(0, 160, Bytes::from(vec![value; 160]), sequence, timestamp)
    }

    fn sequences(frames: &[Frame]) -> Vec<u16> {
        frames
            .iter()
            .filter_map(VoiceReorderBuffer::timing)
            .map(|timing| timing.sequence)
            .collect()
    }

    fn first_payload_bytes(frames: &[Frame]) -> Vec<u8> {
        frames
            .iter()
            .filter_map(|frame| match frame {
                Frame::Voice { data, .. } => data.first().copied(),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn reorder_recovers_swap_dedupes_and_preserves_gap() {
        let mut swapped = VoiceReorderBuffer::new(4);
        let mut output = Vec::new();
        for frame in [
            timed_voice(10, 1000, 10),
            timed_voice(12, 1320, 12),
            timed_voice(11, 1160, 11),
            timed_voice(11, 1160, 11),
            timed_voice(13, 1480, 13),
        ] {
            output.extend(swapped.push(frame));
        }
        assert_eq!(sequences(&output), vec![10, 11, 12, 13]);
        assert_eq!(first_payload_bytes(&output), vec![10, 11, 12, 13]);

        let mut gap = VoiceReorderBuffer::new(4);
        let mut output = Vec::new();
        for frame in [
            timed_voice(20, 2000, 20),
            timed_voice(22, 2320, 22),
            timed_voice(23, 2480, 23),
            timed_voice(24, 2640, 24),
            timed_voice(25, 2800, 25),
        ] {
            output.extend(gap.push(frame));
        }
        assert_eq!(sequences(&output), vec![20, 22, 23, 24, 25]);
        assert_eq!(first_payload_bytes(&output), vec![20, 22, 23, 24, 25]);
        let timestamps: Vec<u32> = output
            .iter()
            .filter_map(VoiceReorderBuffer::timing)
            .map(|timing| timing.timestamp)
            .collect();
        assert_eq!(timestamps, vec![2000, 2320, 2480, 2640, 2800]);
    }

    #[derive(Debug)]
    struct MockDriver {
        name: &'static str,
        incoming: Mutex<mpsc::Receiver<AsteriskResult<Frame>>>,
        written: ParkingMutex<Vec<Frame>>,
        sent_digits: ParkingMutex<Vec<(char, u32)>>,
    }

    impl MockDriver {
        fn new(name: &'static str) -> (Arc<Self>, mpsc::Sender<AsteriskResult<Frame>>) {
            let (sender, receiver) = mpsc::channel(8);
            (
                Arc::new(Self {
                    name,
                    incoming: Mutex::new(receiver),
                    written: ParkingMutex::new(Vec::new()),
                    sent_digits: ParkingMutex::new(Vec::new()),
                }),
                sender,
            )
        }
    }

    #[async_trait::async_trait]
    impl ChannelDriver for MockDriver {
        fn name(&self) -> &str {
            self.name
        }

        fn description(&self) -> &str {
            "media pump test driver"
        }

        async fn request(&self, dest: &str, _caller: Option<&Channel>) -> AsteriskResult<Channel> {
            Ok(Channel::new(format!("{}/{dest}", self.name)))
        }

        async fn call(
            &self,
            channel: &mut Channel,
            _dest: &str,
            _timeout: i32,
        ) -> AsteriskResult<()> {
            channel.state = ChannelState::Up;
            Ok(())
        }

        async fn hangup(&self, channel: &mut Channel) -> AsteriskResult<()> {
            channel.state = ChannelState::Down;
            Ok(())
        }

        async fn answer(&self, channel: &mut Channel) -> AsteriskResult<()> {
            channel.state = ChannelState::Up;
            Ok(())
        }

        async fn read_frame(&self, _channel: &mut Channel) -> AsteriskResult<Frame> {
            self.incoming.lock().await.recv().await.unwrap_or_else(|| {
                Err(AsteriskError::Hangup("test media source ended".into()))
            })
        }

        async fn write_frame(&self, _channel: &mut Channel, frame: &Frame) -> AsteriskResult<()> {
            self.written.lock().push(frame.clone());
            Ok(())
        }

        async fn send_digit_end(
            &self,
            _channel: &mut Channel,
            digit: char,
            duration: u32,
        ) -> AsteriskResult<()> {
            self.sent_digits.lock().push((digit, duration));
            Ok(())
        }
    }

    #[tokio::test]
    async fn cancellation_releases_two_silent_blocked_readers() {
        let (first, _first_sender) = MockDriver::new("FIRST");
        let (second, _second_sender) = MockDriver::new("SECOND");
        let lifetime = BridgeLifetime::new();
        let pumps = start_media_pumps(
            "FIRST/a".into(), first, "SECOND/b".into(), second, lifetime.clone(),
        );

        lifetime.cancel();
        tokio::time::timeout(Duration::from_millis(100), pumps.wait())
            .await
            .expect("silent bridge readers leaked after cancellation");
    }

    #[tokio::test]
    async fn one_reader_exit_cancels_silent_peer() {
        let (first, first_sender) = MockDriver::new("FIRST");
        let (second, _second_sender) = MockDriver::new("SECOND");
        let lifetime = BridgeLifetime::new();
        let pumps = start_media_pumps(
            "FIRST/a".into(), first, "SECOND/b".into(), second, lifetime.clone(),
        );

        drop(first_sender);
        tokio::time::timeout(Duration::from_millis(100), pumps.wait())
            .await
            .expect("peer exit did not cancel the opposite blocked reader");
        assert!(lifetime.is_cancelled());
    }

    #[tokio::test]
    async fn two_pumps_deliver_voice_bytes_in_both_directions() {
        let (first, first_sender) = MockDriver::new("FIRST");
        let (second, second_sender) = MockDriver::new("SECOND");
        let lifetime = BridgeLifetime::new();
        let pumps = start_media_pumps(
            "FIRST/a".into(),
            first.clone(),
            "SECOND/b".into(),
            second.clone(),
            lifetime.clone(),
        );

        first_sender.send(Ok(timed_voice(1, 160, 0x11))).await.unwrap();
        second_sender.send(Ok(timed_voice(50, 8000, 0x22))).await.unwrap();
        tokio::time::timeout(Duration::from_millis(100), async {
            loop {
                if first.written.lock().len() == 1 && second.written.lock().len() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("voice frames did not reach both receiving drivers");

        assert_eq!(first_payload_bytes(&first.written.lock()), vec![0x22]);
        assert_eq!(first_payload_bytes(&second.written.lock()), vec![0x11]);

        lifetime.cancel();
        pumps.wait().await;
    }

    #[tokio::test]
    async fn dtmf_end_crosses_as_a_destination_driver_event() {
        let (first, first_sender) = MockDriver::new("FIRST");
        let (second, _second_sender) = MockDriver::new("SECOND");
        let lifetime = BridgeLifetime::new();
        let pumps = start_media_pumps(
            "FIRST/a".into(),
            first,
            "SECOND/b".into(),
            second.clone(),
            lifetime.clone(),
        );

        first_sender
            .send(Ok(Frame::dtmf_end('5', 240)))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_millis(100), async {
            loop {
                if second.sent_digits.lock().as_slice() == [('5', 240)] {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("DTMF event did not reach the receiving driver");

        assert!(second.written.lock().is_empty());
        lifetime.cancel();
        pumps.wait().await;
    }
}
