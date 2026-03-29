//! Frame read and write pipelines -- the heart of Asterisk channel I/O.
//!
//! Mirrors C Asterisk's `ast_read()` (channel.c ~3553-4260) and
//! `ast_write()` (channel.c ~5180-5430).
//!
//! These functions orchestrate the full pipeline:
//! hangup check -> frame queue -> driver read -> DTMF emulation ->
//! framehooks -> audiohooks -> generator mixing -> format translation.

use asterisk_types::{AsteriskError, AsteriskResult, Frame};

use super::dtmf::DtmfEndAction;
use super::Channel;

/// Read a frame from a channel, processing it through the full pipeline.
///
/// This is the Rust equivalent of C Asterisk's `__ast_read()`.
///
/// Pipeline order:
/// 1. Check hangup -- return `None` if channel should hang up.
/// 2. Dequeue from internal `frame_queue` (queued frames have priority).
/// 3. If no queued frame, call `read_fn` to get a frame from the channel driver.
/// 4. Process DTMF timing (begin/end emulation, minimum duration enforcement).
/// 5. Apply framehooks (read event).
/// 6. Process audiohooks (read direction).
/// 7. If generator is active and frame is Voice, mix generator output.
/// 8. Return the processed frame.
///
/// # Arguments
/// * `channel` -- the channel to read from (must be locked by caller)
/// * `read_fn` -- closure that calls the channel driver's `read_frame`.
///   We take a closure instead of the driver directly because the channel
///   is already borrowed mutably.
///
/// Returns `Ok(Some(frame))` on success, `Ok(None)` on hangup, or `Err` on error.
pub fn channel_read<F>(channel: &mut Channel, read_fn: F) -> AsteriskResult<Option<Frame>>
where
    F: FnOnce(&mut Channel) -> AsteriskResult<Frame>,
{
    // 1. Check hangup
    if channel.check_hangup() {
        // Deactivate any active generator on hangup
        channel.generator.deactivate();
        tracing::debug!(channel = %channel.name, "channel_read: hangup detected");
        return Ok(None);
    }

    // 2. Dequeue from frame_queue (queued frames have priority)
    let frame = if let Some(queued) = channel.dequeue_frame() {
        queued
    } else {
        // 3. Call driver read
        match read_fn(channel) {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!(channel = %channel.name, error = %e, "driver read error");
                return Err(e);
            }
        }
    };

    // Process the frame through the pipeline
    process_read_frame(channel, frame)
}

/// Process a frame through steps 4-8 of the read pipeline.
fn process_read_frame(channel: &mut Channel, mut frame: Frame) -> AsteriskResult<Option<Frame>> {
    // 4. DTMF handling
    frame = match frame {
        Frame::DtmfBegin { digit } => {
            if channel.dtmf_state.on_begin(digit) {
                Frame::DtmfBegin { digit }
            } else {
                Frame::Null
            }
        }
        Frame::DtmfEnd { digit, duration_ms } => {
            match channel.dtmf_state.on_end(digit, duration_ms) {
                DtmfEndAction::Passthrough { duration_ms: dur } => {
                    // Notify generator of digit if active
                    channel.generator.digit(digit);
                    Frame::DtmfEnd {
                        digit,
                        duration_ms: dur,
                    }
                }
                DtmfEndAction::EmulateBeginFirst { .. } => {
                    // Turn this into a DTMF_BEGIN; the emulated end will
                    // come later via check_emulation_tick on null/voice frames.
                    Frame::DtmfBegin { digit }
                }
                DtmfEndAction::EmulateRemaining { .. } => {
                    // Suppress the end frame; emulation timer will produce it.
                    Frame::Null
                }
                DtmfEndAction::Defer => {
                    // Re-queue for later delivery.
                    channel.queue_frame(Frame::DtmfEnd { digit, duration_ms });
                    Frame::Null
                }
            }
        }
        Frame::Null => {
            // Check DTMF emulation tick on null frames.
            if let Some((digit, dur)) = channel.dtmf_state.check_emulation_tick() {
                tracing::debug!(digit = %digit, duration = dur, "DTMF end emulated");
                Frame::DtmfEnd {
                    digit,
                    duration_ms: dur,
                }
            } else {
                Frame::Null
            }
        }
        Frame::Voice { .. } => {
            // Check DTMF emulation on voice frames too.
            if let Some((digit, dur)) = channel.dtmf_state.check_emulation_tick() {
                // Replace the voice frame with the emulated DTMF end.
                tracing::debug!(digit = %digit, duration = dur, "DTMF end emulated (from voice)");
                Frame::DtmfEnd {
                    digit,
                    duration_ms: dur,
                }
            } else if channel.dtmf_state.emulating {
                // Currently emulating a digit -- drop voice frames to maintain gap.
                Frame::Null
            } else {
                frame
            }
        }
        other => other,
    };

    // 5. Framehooks (read event)
    if !channel.framehooks.is_empty() {
        match channel.framehooks.process_read(&frame) {
            Some(f) => frame = f,
            None => return Ok(Some(Frame::Null)),
        }
    }

    // 6. Audiohooks (read direction)
    if !channel.audiohooks.is_empty() {
        match channel.audiohooks.process_read(&frame) {
            Some(f) => frame = f,
            None => return Ok(Some(Frame::Null)),
        }
    }

    // 7. Generator mixing
    // If we have an active generator and the frame is voice, we can
    // either replace or mix the generator output.  In C Asterisk, when
    // a generator is active, voice frames trigger `ast_read_generator_actions`
    // which calls the generator to produce audio.  For simplicity we let the
    // generator's frame replace voice silence or stand alone.
    if channel.generator.is_active() {
        if let Frame::Voice { samples, .. } = &frame {
            if let Some(gen_frame) = channel.generator.generate(*samples as usize) {
                // Use generator output instead of the driver's voice frame.
                frame = gen_frame;
            }
        }
    }

    // 8. Format translation would happen here if we had a transcoding
    // subsystem.  For now, frames pass through in their native format.

    Ok(Some(frame))
}

/// Write a frame to a channel, processing it through the full pipeline.
///
/// This is the Rust equivalent of C Asterisk's `ast_write()`.
///
/// Pipeline order:
/// 1. Check hangup -- return error if channel should hang up.
/// 2. Apply framehooks (write event).
/// 3. Process audiohooks (write direction).
/// 4. Handle generator interaction: if a generator is active and a voice
///    frame arrives, either deactivate the generator (if WRITE_INT is set)
///    or silently consume the frame.
/// 5. For Voice frames: format translation would apply here.
/// 6. Call `write_fn` to send the frame to the channel driver.
///
/// # Arguments
/// * `channel` -- the channel to write to
/// * `frame` -- the frame to write
/// * `write_fn` -- closure that calls the channel driver's `write_frame`
pub fn channel_write<F>(
    channel: &mut Channel,
    frame: &Frame,
    write_fn: F,
) -> AsteriskResult<()>
where
    F: FnOnce(&mut Channel, &Frame) -> AsteriskResult<()>,
{
    // 1. Check hangup
    if channel.check_hangup() {
        return Err(AsteriskError::Hangup(format!(
            "channel {} is hanging up",
            channel.name
        )));
    }

    let mut owned_frame;
    let mut current: &Frame = frame;

    // 2. Framehooks (write event)
    if !channel.framehooks.is_empty() {
        match channel.framehooks.process_write(frame) {
            Some(f) => {
                owned_frame = f;
                current = &owned_frame;
            }
            None => return Ok(()), // framehook consumed the frame
        }
    }

    // 3. Audiohooks (write direction)
    if !channel.audiohooks.is_empty() {
        match channel.audiohooks.process_write(current) {
            Some(f) => {
                owned_frame = f;
                current = &owned_frame;
            }
            None => return Ok(()), // audiohook consumed the frame
        }
    }

    // 4. Generator interaction
    if channel.generator.is_active() {
        match current {
            Frame::DtmfEnd { digit, .. } => {
                // Pass DTMF end through even with active generator
                // (inband DTMF detection needs it)
                channel.generator.digit(*digit);
            }
            Frame::Voice { .. } => {
                if channel.flags.contains(asterisk_types::ChannelFlags::WRITE_INT) {
                    // WRITE_INT flag: deactivate generator when voice is written
                    channel.generator.deactivate();
                } else {
                    // Generator is active -- silently consume the voice frame
                    // (matching C behavior where non-WRITE_INT voice writes
                    // are dropped while a generator runs)
                    return Ok(());
                }
            }
            _ => {}
        }
    }

    // 5. Format translation would apply here for voice frames.

    // 6. Call driver write
    write_fn(channel, current)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    fn make_voice_frame() -> Frame {
        Frame::voice(0, 160, Bytes::from(vec![0xFFu8; 160]))
    }

    #[test]
    fn read_returns_none_on_hangup() {
        let mut ch = Channel::new("Test/hangup-read");
        ch.softhangup(super::super::softhangup::AST_SOFTHANGUP_DEV);

        let result = channel_read(&mut ch, |_| Ok(Frame::Null));
        assert!(result.is_ok());
        assert!(result.unwrap().is_none(), "should return None on hangup");
    }

    #[test]
    fn read_dequeues_before_driver() {
        let mut ch = Channel::new("Test/queue-read");
        let queued = Frame::text("hello".to_string());
        ch.queue_frame(queued);

        let result = channel_read(&mut ch, |_| {
            panic!("driver should not be called when queue has frames");
        });

        assert!(result.is_ok());
        let frame = result.unwrap().unwrap();
        assert!(matches!(frame, Frame::Text { text } if text == "hello"));
    }

    #[test]
    fn read_calls_driver_when_queue_empty() {
        let mut ch = Channel::new("Test/driver-read");

        let result = channel_read(&mut ch, |_| Ok(make_voice_frame()));
        assert!(result.is_ok());
        let frame = result.unwrap().unwrap();
        assert!(frame.is_voice());
    }

    #[test]
    fn write_fails_on_hangup() {
        let mut ch = Channel::new("Test/hangup-write");
        ch.softhangup(super::super::softhangup::AST_SOFTHANGUP_DEV);

        let frame = make_voice_frame();
        let result = channel_write(&mut ch, &frame, |_, _| Ok(()));
        assert!(result.is_err());
    }

    #[test]
    fn write_passes_through() {
        let mut ch = Channel::new("Test/write-ok");
        let frame = make_voice_frame();

        let mut written = false;
        let result = channel_write(&mut ch, &frame, |_, f| {
            assert!(f.is_voice());
            written = true;
            Ok(())
        });
        assert!(result.is_ok());
        assert!(written, "driver write should have been called");
    }

    #[test]
    fn write_consumed_by_generator_without_write_int() {
        let mut ch = Channel::new("Test/gen-consume");

        // Install a simple generator
        struct DummyGen;
        impl super::super::generator::Generator for DummyGen {
            fn generate(&mut self, _samples: usize) -> Option<Frame> {
                Some(Frame::Null)
            }
        }
        ch.generator.activate(Box::new(DummyGen));

        let frame = make_voice_frame();
        let result = channel_write(&mut ch, &frame, |_, _| {
            panic!("driver write should not be called when generator is active");
        });
        assert!(result.is_ok());
    }

    #[test]
    fn framehook_in_read_pipeline() {
        let mut ch = Channel::new("Test/fh-read");

        // Install a framehook that turns voice frames into null
        ch.framehooks.attach(Box::new(|frame, _event| {
            if frame.is_voice() {
                Some(Frame::Null)
            } else {
                Some(frame.clone())
            }
        }));

        let result = channel_read(&mut ch, |_| Ok(make_voice_frame()));
        assert!(result.is_ok());
        let frame = result.unwrap().unwrap();
        assert!(matches!(frame, Frame::Null));
    }
}
