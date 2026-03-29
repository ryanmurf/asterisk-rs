//! DTMF emulation and timing enforcement.
//!
//! Mirrors the DTMF handling in C Asterisk's `ast_read()` (channel.c ~3897-4085).
//! When a channel driver does not generate proper DTMF begin/end pairs, or when
//! digits arrive too quickly, this module fills in the gaps.

use std::time::Instant;

/// Minimum gap between the end of one DTMF digit and the beginning of the
/// next, in milliseconds.  Matches `AST_MIN_DTMF_GAP` in channel.c.
pub const AST_MIN_DTMF_GAP_MS: u32 = 45;

/// Minimum acceptable DTMF duration, in milliseconds.
/// Digits shorter than this are stretched via emulation.
pub const AST_MIN_DTMF_DURATION_MS: u32 = 80;

/// Default DTMF emulation duration when a driver does not supply one.
/// Matches `AST_DEFAULT_EMULATE_DTMF_DURATION` in channel.c.
pub const AST_DEFAULT_DTMF_DURATION_MS: u32 = 100;

/// Maximum DTMF duration we'll track, in milliseconds.
pub const AST_MAX_DTMF_DURATION_MS: u32 = 8000;

/// Per-channel DTMF emulation state.
///
/// Tracks whether we are currently in the middle of receiving a DTMF digit,
/// whether emulation is active, and the minimum-duration/gap enforcement
/// timestamps.
#[derive(Debug, Clone)]
pub struct DtmfState {
    /// True when a `DTMF_BEGIN` has been received but no matching `DTMF_END`.
    pub in_dtmf: bool,

    /// True when we are emulating a DTMF end (the channel driver only sent a
    /// begin, or the digit was too short and we need to pad it).
    pub emulating: bool,

    /// The digit currently being emulated.
    pub digit: char,

    /// Timestamp when the current DTMF digit started (begin received).
    pub begin_time: Option<Instant>,

    /// Remaining emulation duration in milliseconds.
    /// When this reaches 0 (and a voice/null frame arrives), we emit the
    /// synthesized `DTMF_END`.
    pub emulate_duration_ms: u32,

    /// Timestamp of the last `DTMF_END` -- used to enforce `AST_MIN_DTMF_GAP`.
    pub last_end_time: Option<Instant>,
}

impl Default for DtmfState {
    fn default() -> Self {
        Self::new()
    }
}

impl DtmfState {
    pub fn new() -> Self {
        Self {
            in_dtmf: false,
            emulating: false,
            digit: '\0',
            begin_time: None,
            emulate_duration_ms: 0,
            last_end_time: None,
        }
    }

    /// Returns `true` if a new DTMF begin should be suppressed because we are
    /// still within the minimum gap from the previous digit, or because we are
    /// already emulating.
    pub fn should_suppress_begin(&self) -> bool {
        if self.emulating {
            return true;
        }
        if let Some(last_end) = self.last_end_time {
            let elapsed = last_end.elapsed().as_millis() as u32;
            if elapsed < AST_MIN_DTMF_GAP_MS {
                return true;
            }
        }
        false
    }

    /// Called when a `DTMF_BEGIN` frame is received from the driver.
    /// Returns `true` if the begin should be passed through, `false` if it
    /// should be suppressed (turned into a null frame by the caller).
    pub fn on_begin(&mut self, digit: char) -> bool {
        if self.should_suppress_begin() {
            tracing::debug!(digit = %digit, "DTMF begin suppressed (gap/emulation)");
            return false;
        }
        self.in_dtmf = true;
        self.digit = digit;
        self.begin_time = Some(Instant::now());
        true
    }

    /// Called when a `DTMF_END` frame is received from the driver.
    ///
    /// Returns one of:
    /// - `DtmfEndAction::Passthrough { duration_ms }` -- let the frame through (possibly with adjusted duration).
    /// - `DtmfEndAction::EmulateBeginFirst { emulate_duration_ms }` -- we never saw a begin; turn this end into a begin and start emulation.
    /// - `DtmfEndAction::EmulateRemaining { remaining_ms }` -- digit was too short; swallow the end and emulate the rest.
    /// - `DtmfEndAction::Defer` -- too soon after the last digit; queue it.
    pub fn on_end(&mut self, digit: char, driver_duration_ms: u32) -> DtmfEndAction {
        let now = Instant::now();

        // If we were IN_DTMF (got a begin), compute actual duration.
        if self.in_dtmf {
            self.in_dtmf = false;
            let actual_ms = self
                .begin_time
                .map(|t| now.duration_since(t).as_millis() as u32)
                .unwrap_or(driver_duration_ms);
            let duration = if driver_duration_ms > 0 {
                driver_duration_ms
            } else {
                actual_ms
            };

            if duration < AST_MIN_DTMF_DURATION_MS {
                // Too short -- need to emulate the remainder.
                let remaining = AST_MIN_DTMF_DURATION_MS.saturating_sub(duration);
                self.emulating = true;
                self.digit = digit;
                self.emulate_duration_ms = remaining;
                // begin_time stays as-is so the total emulated length is correct
                return DtmfEndAction::EmulateRemaining { remaining_ms: remaining };
            }

            // Good duration -- pass through.
            let final_duration = duration.max(AST_MIN_DTMF_DURATION_MS);
            self.last_end_time = Some(now);
            return DtmfEndAction::Passthrough {
                duration_ms: final_duration,
            };
        }

        // No begin seen -- check gap.
        if let Some(last_end) = self.last_end_time {
            let gap = now.duration_since(last_end).as_millis() as u32;
            if gap < AST_MIN_DTMF_GAP_MS {
                return DtmfEndAction::Defer;
            }
        }

        // No begin was seen at all. Convert to a begin + emulate the end.
        let emulate_dur = if driver_duration_ms > AST_MIN_DTMF_DURATION_MS {
            driver_duration_ms
        } else {
            AST_DEFAULT_DTMF_DURATION_MS
        };

        self.emulating = true;
        self.digit = digit;
        self.begin_time = Some(now);
        self.emulate_duration_ms = emulate_dur;

        DtmfEndAction::EmulateBeginFirst {
            emulate_duration_ms: emulate_dur,
        }
    }

    /// Called on each null/voice frame tick while `emulating` is true.
    ///
    /// Returns `Some((digit, duration_ms))` when it is time to emit the
    /// synthetic `DTMF_END`, or `None` if more time is needed.
    pub fn check_emulation_tick(&mut self) -> Option<(char, u32)> {
        if !self.emulating {
            return None;
        }

        // Has the emulation time elapsed?
        if self.emulate_duration_ms == 0 {
            // Duration was zeroed on a previous tick; now clear the flag
            // (we wait one extra frame to ensure a gap).
            self.emulating = false;
            self.digit = '\0';
            return None;
        }

        let elapsed = self
            .begin_time
            .map(|t| t.elapsed().as_millis() as u32)
            .unwrap_or(0);

        if elapsed >= self.emulate_duration_ms {
            let digit = self.digit;
            let duration = elapsed;
            self.emulate_duration_ms = 0;
            self.last_end_time = Some(Instant::now());
            // Don't clear `emulating` yet -- we wait one more frame tick
            // to ensure the gap requirement is met.
            Some((digit, duration))
        } else {
            None
        }
    }
}

/// What the read pipeline should do after `DtmfState::on_end`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DtmfEndAction {
    /// Pass the DTMF_END frame through with the given (possibly adjusted) duration.
    Passthrough { duration_ms: u32 },
    /// No begin was received; convert the end into a begin frame and start
    /// emulation of the end after `emulate_duration_ms`.
    EmulateBeginFirst { emulate_duration_ms: u32 },
    /// Begin was received but the digit was too short.  Suppress the end and
    /// emulate `remaining_ms` more.
    EmulateRemaining { remaining_ms: u32 },
    /// Too soon after the previous digit -- defer/queue this frame.
    Defer,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn begin_end_passthrough() {
        let mut state = DtmfState::new();
        assert!(state.on_begin('1'));
        // Simulate time passing > min duration
        thread::sleep(Duration::from_millis(AST_MIN_DTMF_DURATION_MS as u64 + 10));
        match state.on_end('1', 0) {
            DtmfEndAction::Passthrough { duration_ms } => {
                assert!(duration_ms >= AST_MIN_DTMF_DURATION_MS);
            }
            other => panic!("expected Passthrough, got {:?}", other),
        }
    }

    #[test]
    fn end_without_begin_triggers_emulation() {
        let mut state = DtmfState::new();
        match state.on_end('5', 0) {
            DtmfEndAction::EmulateBeginFirst { emulate_duration_ms } => {
                assert!(emulate_duration_ms >= AST_DEFAULT_DTMF_DURATION_MS);
            }
            other => panic!("expected EmulateBeginFirst, got {:?}", other),
        }
        assert!(state.emulating);
    }

    #[test]
    fn short_digit_triggers_emulation() {
        let mut state = DtmfState::new();
        assert!(state.on_begin('3'));
        // End immediately (< min duration)
        match state.on_end('3', 10) {
            DtmfEndAction::EmulateRemaining { remaining_ms } => {
                assert!(remaining_ms > 0);
            }
            other => panic!("expected EmulateRemaining, got {:?}", other),
        }
    }

    #[test]
    fn gap_enforcement_suppresses_begin() {
        let mut state = DtmfState::new();
        // Simulate a completed digit
        state.last_end_time = Some(Instant::now());
        // Immediately try another begin
        assert!(!state.on_begin('2'), "should suppress due to gap");
    }

    #[test]
    fn emulation_tick_fires() {
        let mut state = DtmfState::new();
        // Set up emulation with very short duration
        state.emulating = true;
        state.digit = '7';
        state.begin_time = Some(Instant::now() - Duration::from_millis(200));
        state.emulate_duration_ms = 100;

        let result = state.check_emulation_tick();
        assert!(result.is_some());
        let (digit, dur) = result.unwrap();
        assert_eq!(digit, '7');
        assert!(dur >= 100);
    }
}
