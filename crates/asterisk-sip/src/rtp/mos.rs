//! Real-time MOS (Mean Opinion Score) estimation from RTP statistics.
//!
//! Implements the ITU-T G.107 E-model for voice quality estimation.
//! This provides objective call quality metrics derived from network
//! impairments (delay, jitter, packet loss) and codec characteristics.
//!
//! The E-model computes an R-factor (0-100) which maps to a MOS score
//! (1.0-4.5). This is used for real-time quality monitoring during calls
//! and for CDR/reporting at call end.

use super::RtpSession;
use std::fmt;

// ---------------------------------------------------------------------------
// E-model constants (ITU-T G.107)
// ---------------------------------------------------------------------------

/// Default signal-to-noise ratio (Ro) for G.107 default conditions.
const RO_DEFAULT: f64 = 94.768;

/// Simultaneous impairment factor (Is) - default value.
const IS_DEFAULT: f64 = 1.41;

/// Advantage factor for VoIP (landline = 0, cellular = 5, satellite = 10).
/// We use 0 (most conservative) for general VoIP.
const ADVANTAGE_FACTOR: f64 = 0.0;

/// Assumed jitter buffer delay contribution in ms (on top of network delay).
/// A typical adaptive jitter buffer adds approximately 2x the measured jitter.
const JITTER_BUFFER_FACTOR: f64 = 2.0;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Codec types with their associated E-model impairment parameters.
///
/// Each codec has:
/// - `Ie`: Equipment impairment factor at zero packet loss
/// - `Bpl`: Packet loss robustness factor (codec-specific)
///
/// Values are sourced from ITU-T G.113 Appendix I.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CodecType {
    /// G.711 u-law (PCMU) - Ie=0, Bpl=25.1
    G711Ulaw,
    /// G.711 A-law (PCMA) - Ie=0, Bpl=25.1
    G711Alaw,
    /// G.729 / G.729A - Ie=11, Bpl=19
    G729,
    /// G.722 wideband - Ie=0, Bpl=25.1
    G722,
    /// Opus (estimated) - Ie=0, Bpl=20
    Opus,
    /// GSM Full Rate - Ie=20, Bpl=17
    GSM,
    /// iLBC - Ie=11, Bpl=20
    ILBC,
    /// Speex - Ie=11, Bpl=20
    Speex,
    /// Unknown codec (conservative defaults) - Ie=20, Bpl=10
    Unknown,
}

impl CodecType {
    /// Equipment impairment factor at zero packet loss (Ie).
    ///
    /// Higher values mean more intrinsic codec distortion.
    pub fn ie(&self) -> f64 {
        match self {
            CodecType::G711Ulaw => 0.0,
            CodecType::G711Alaw => 0.0,
            CodecType::G729 => 11.0,
            CodecType::G722 => 0.0,
            CodecType::Opus => 0.0,
            CodecType::GSM => 20.0,
            CodecType::ILBC => 11.0,
            CodecType::Speex => 11.0,
            CodecType::Unknown => 20.0,
        }
    }

    /// Packet loss robustness factor (Bpl).
    ///
    /// Higher values mean the codec degrades more gracefully under loss.
    pub fn bpl(&self) -> f64 {
        match self {
            CodecType::G711Ulaw => 25.1,
            CodecType::G711Alaw => 25.1,
            CodecType::G729 => 19.0,
            CodecType::G722 => 25.1,
            CodecType::Opus => 20.0,
            CodecType::GSM => 17.0,
            CodecType::ILBC => 20.0,
            CodecType::Speex => 20.0,
            CodecType::Unknown => 10.0,
        }
    }
}

impl fmt::Display for CodecType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CodecType::G711Ulaw => write!(f, "G.711 u-law"),
            CodecType::G711Alaw => write!(f, "G.711 A-law"),
            CodecType::G729 => write!(f, "G.729"),
            CodecType::G722 => write!(f, "G.722"),
            CodecType::Opus => write!(f, "Opus"),
            CodecType::GSM => write!(f, "GSM"),
            CodecType::ILBC => write!(f, "iLBC"),
            CodecType::Speex => write!(f, "Speex"),
            CodecType::Unknown => write!(f, "Unknown"),
        }
    }
}

/// RTP stream metrics used as input to the E-model calculation.
#[derive(Debug, Clone)]
pub struct RtpMetrics {
    /// Round-trip time in milliseconds.
    pub rtt_ms: f64,
    /// One-way delay in milliseconds (estimated as RTT/2 if not measured directly).
    pub delay_ms: f64,
    /// Jitter in milliseconds (interarrival jitter from RTCP).
    pub jitter_ms: f64,
    /// Packet loss percentage (0.0 - 100.0).
    pub packet_loss_pct: f64,
    /// Codec in use.
    pub codec: CodecType,
    /// Total packets received.
    pub packets_received: u64,
    /// Total packets lost.
    pub packets_lost: u64,
}

impl RtpMetrics {
    /// Create metrics from raw statistics.
    ///
    /// Computes `delay_ms` as `rtt_ms / 2` and `packet_loss_pct` from
    /// received/lost counts.
    pub fn from_stats(
        rtt_ms: f64,
        jitter_ms: f64,
        packets_received: u64,
        packets_lost: u64,
        codec: CodecType,
    ) -> Self {
        let total = packets_received + packets_lost;
        let packet_loss_pct = if total > 0 {
            (packets_lost as f64 / total as f64) * 100.0
        } else {
            0.0
        };
        Self {
            rtt_ms,
            delay_ms: rtt_ms / 2.0,
            jitter_ms,
            packet_loss_pct,
            codec,
            packets_received,
            packets_lost,
        }
    }
}

/// Voice quality rating derived from MOS/R-factor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityRating {
    /// Excellent quality (MOS >= 4.3, R >= 90). Users very satisfied.
    Excellent,
    /// Good quality (MOS >= 4.0, R >= 80). Users satisfied.
    Good,
    /// Fair quality (MOS >= 3.6, R >= 70). Some users dissatisfied.
    Fair,
    /// Poor quality (MOS >= 3.1, R >= 60). Many users dissatisfied.
    Poor,
    /// Bad quality (MOS < 3.1, R < 60). Nearly all users dissatisfied.
    Bad,
}

impl QualityRating {
    /// Derive rating from R-factor.
    pub fn from_r_factor(r: f64) -> Self {
        if r >= 90.0 {
            QualityRating::Excellent
        } else if r >= 80.0 {
            QualityRating::Good
        } else if r >= 70.0 {
            QualityRating::Fair
        } else if r >= 60.0 {
            QualityRating::Poor
        } else {
            QualityRating::Bad
        }
    }
}

impl fmt::Display for QualityRating {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QualityRating::Excellent => write!(f, "Excellent"),
            QualityRating::Good => write!(f, "Good"),
            QualityRating::Fair => write!(f, "Fair"),
            QualityRating::Poor => write!(f, "Poor"),
            QualityRating::Bad => write!(f, "Bad"),
        }
    }
}

/// Computed call quality metrics.
#[derive(Debug, Clone)]
pub struct CallQuality {
    /// Mean Opinion Score (1.0 = bad, 4.5 = excellent).
    pub mos: f64,
    /// R-factor from the E-model (0-100).
    pub r_factor: f64,
    /// Categorical quality rating.
    pub quality: QualityRating,
    /// The input metrics used for this computation.
    pub metrics: RtpMetrics,
}

impl fmt::Display for CallQuality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "MOS={:.2} R={:.1} ({}) [loss={:.1}% delay={:.0}ms jitter={:.0}ms codec={}]",
            self.mos,
            self.r_factor,
            self.quality,
            self.metrics.packet_loss_pct,
            self.metrics.delay_ms,
            self.metrics.jitter_ms,
            self.metrics.codec,
        )
    }
}

/// Real-time MOS score estimator using the ITU-T G.107 E-model.
///
/// Computes call quality from RTP stream statistics including delay,
/// jitter, packet loss, and codec characteristics.
///
/// # Example
///
/// ```
/// use asterisk_sip::rtp::mos::{MosEstimator, CodecType, RtpMetrics};
///
/// let estimator = MosEstimator::new(CodecType::G711Ulaw);
/// let metrics = RtpMetrics {
///     rtt_ms: 40.0,
///     delay_ms: 20.0,
///     jitter_ms: 5.0,
///     packet_loss_pct: 0.0,
///     codec: CodecType::G711Ulaw,
///     packets_received: 1000,
///     packets_lost: 0,
/// };
/// let quality = estimator.estimate(&metrics);
/// assert!(quality.mos > 4.0);
/// ```
pub struct MosEstimator {
    /// Codec-specific impairment factor (Ie).
    codec_impairment: f64,
    /// Codec packet loss robustness factor (Bpl).
    codec_bpl: f64,
    /// The current codec type.
    codec: CodecType,
}

impl MosEstimator {
    /// Create a new MOS estimator configured for the given codec.
    pub fn new(codec: CodecType) -> Self {
        Self {
            codec_impairment: codec.ie(),
            codec_bpl: codec.bpl(),
            codec,
        }
    }

    /// Update the codec type (e.g., after SDP renegotiation).
    pub fn set_codec(&mut self, codec: CodecType) {
        self.codec_impairment = codec.ie();
        self.codec_bpl = codec.bpl();
        self.codec = codec;
    }

    /// Get the currently configured codec.
    pub fn codec(&self) -> CodecType {
        self.codec
    }

    /// Compute full call quality metrics from RTP statistics.
    pub fn estimate(&self, metrics: &RtpMetrics) -> CallQuality {
        let r_factor = self.compute_r_factor(
            metrics.delay_ms,
            metrics.jitter_ms,
            metrics.packet_loss_pct,
        );
        let mos = r_factor_to_mos(r_factor);
        let quality = QualityRating::from_r_factor(r_factor);

        CallQuality {
            mos,
            r_factor,
            quality,
            metrics: metrics.clone(),
        }
    }

    /// Convenience method: compute just the MOS score from basic parameters.
    ///
    /// Uses RTT/2 as one-way delay. Negative, NaN, and infinite inputs are
    /// sanitized (clamped to safe ranges) rather than causing panics or
    /// producing NaN results.
    pub fn mos_score(&self, rtt_ms: f64, jitter_ms: f64, loss_pct: f64) -> f64 {
        let one_way_delay = sanitize_non_negative(rtt_ms) / 2.0;
        let r = self.compute_r_factor(one_way_delay, sanitize_non_negative(jitter_ms), loss_pct);
        r_factor_to_mos(r)
    }

    /// Compute the R-factor per ITU-T G.107.
    ///
    /// ```text
    /// R = Ro - Is - Id - Ie_eff + A
    /// ```
    ///
    /// Inputs are sanitized: delay and jitter are clamped to >= 0,
    /// loss is clamped to [0, 100]. NaN/Infinity are treated as 0
    /// (for delay/jitter) or 100 (for loss).
    fn compute_r_factor(&self, one_way_delay_ms: f64, jitter_ms: f64, loss_pct: f64) -> f64 {
        let delay = sanitize_non_negative(one_way_delay_ms);
        let jitter = sanitize_non_negative(jitter_ms);
        let loss = sanitize_loss(loss_pct);

        let id = self.compute_delay_impairment(delay, jitter);
        let ie_eff = self.compute_equipment_impairment(loss);
        let r = RO_DEFAULT - IS_DEFAULT - id - ie_eff + ADVANTAGE_FACTOR;
        // Clamp to valid range.
        r.clamp(0.0, 100.0)
    }

    /// Compute the delay impairment factor (Id).
    ///
    /// ```text
    /// d = one_way_delay + jitter_buffer_delay
    /// H(x) = 1 if x > 0, else 0
    /// Id = 0.024 * d + 0.11 * (d - 177.3) * H(d - 177.3)
    /// ```
    ///
    /// The jitter buffer adds approximately `JITTER_BUFFER_FACTOR * jitter` ms.
    /// Inputs must already be sanitized (non-negative, finite).
    fn compute_delay_impairment(&self, one_way_delay_ms: f64, jitter_ms: f64) -> f64 {
        // Total mouth-to-ear delay includes:
        // - One-way network delay (half of RTT)
        // - Jitter buffer delay (approximately 2x measured jitter)
        let d = one_way_delay_ms + (JITTER_BUFFER_FACTOR * jitter_ms);

        // H(x) = Heaviside step function
        let excess = d - 177.3;
        let h = if excess > 0.0 { 1.0 } else { 0.0 };

        0.024 * d + 0.11 * excess * h
    }

    /// Compute the effective equipment impairment factor (Ie_eff).
    ///
    /// ```text
    /// Ie_eff = Ie + (95 - Ie) * Ppl / (Ppl + Bpl)
    /// ```
    ///
    /// Input `packet_loss_pct` must already be sanitized to [0, 100].
    fn compute_equipment_impairment(&self, packet_loss_pct: f64) -> f64 {
        let ie = self.codec_impairment;
        let bpl = self.codec_bpl;
        let ppl = packet_loss_pct;

        let denom = ppl + bpl;
        if denom <= 0.0 {
            // Guard against division by zero. With sanitized inputs (ppl >= 0,
            // bpl > 0) this should never trigger, but defend in depth.
            return ie;
        }

        ie + (95.0 - ie) * ppl / denom
    }
}

/// Sanitize a value that must be non-negative (delay, jitter, RTT).
/// NaN and negative values become 0.0; Infinity is left as-is (the
/// downstream arithmetic will drive R to 0 and MOS to 1.0).
#[inline]
fn sanitize_non_negative(v: f64) -> f64 {
    if v.is_nan() || v < 0.0 {
        0.0
    } else {
        v
    }
}

/// Sanitize a packet-loss percentage to [0.0, 100.0].
/// NaN and Infinity are treated as 100.0 (worst case).
#[inline]
fn sanitize_loss(v: f64) -> f64 {
    if v.is_nan() || v.is_infinite() {
        if v == f64::NEG_INFINITY {
            return 0.0;
        }
        return 100.0;
    }
    v.clamp(0.0, 100.0)
}

/// Convert R-factor to MOS score per ITU-T G.107.
///
/// ```text
/// If R < 0:   MOS = 1.0
/// If R > 100: MOS = 4.5
/// Else:       MOS = 1 + 0.035 * R + R * (R - 60) * (100 - R) * 7e-6
/// ```
///
/// Note: The raw polynomial can produce values slightly below 1.0 for
/// very small positive R values (~0 to ~6.5). Per ITU-T G.107, the
/// result is clamped to [1.0, 4.5].
pub fn r_factor_to_mos(r: f64) -> f64 {
    if !r.is_finite() || r < 0.0 {
        // NaN, -Infinity, and negative values all map to worst-case MOS.
        // +Infinity is treated as R > 100 which maps to 4.5.
        if r == f64::INFINITY {
            return 4.5;
        }
        1.0
    } else if r > 100.0 {
        4.5
    } else {
        let mos = 1.0 + 0.035 * r + r * (r - 60.0) * (100.0 - r) * 7.0e-6;
        mos.clamp(1.0, 4.5)
    }
}

/// Convert MOS score back to approximate R-factor.
///
/// This is an approximate inverse of the MOS-to-R mapping.
/// Uses a simple bisection search since the MOS formula is monotonic.
pub fn mos_to_r_factor(mos: f64) -> f64 {
    if mos.is_nan() || mos <= 1.0 {
        return 0.0;
    }
    if !mos.is_finite() || mos >= 4.5 {
        // +Infinity maps to R=100 (best possible).
        // -Infinity was already caught by the <= 1.0 check above.
        return 100.0;
    }
    // Bisection search
    let mut lo = 0.0_f64;
    let mut hi = 100.0_f64;
    for _ in 0..50 {
        let mid = (lo + hi) / 2.0;
        if r_factor_to_mos(mid) < mos {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (lo + hi) / 2.0
}

// ---------------------------------------------------------------------------
// RtpSession integration
// ---------------------------------------------------------------------------

impl RtpSession {
    /// Compute current call quality from this session's statistics.
    ///
    /// Returns `None` if no packets have been received yet.
    /// Since `RtpSession` does not track RTT/jitter natively, callers
    /// should use `MosEstimator::estimate()` with full `RtpMetrics` for
    /// accurate results. This method uses conservative defaults.
    pub fn current_quality(&self) -> Option<CallQuality> {
        let received = self.stats.packets_received.load(std::sync::atomic::Ordering::Relaxed);
        if received == 0 {
            return None;
        }

        // RtpSession doesn't track these natively; use conservative defaults.
        // For production use, the RTCP stack should feed actual values into
        // MosEstimator::estimate() directly.
        let metrics = RtpMetrics {
            rtt_ms: 0.0,
            delay_ms: 0.0,
            jitter_ms: 0.0,
            packet_loss_pct: 0.0,
            codec: CodecType::G711Ulaw,
            packets_received: received as u64,
            packets_lost: 0,
        };

        let estimator = MosEstimator::new(CodecType::G711Ulaw);
        Some(estimator.estimate(&metrics))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: compute MOS for G.711 u-law with given parameters.
    fn g711_mos(rtt_ms: f64, jitter_ms: f64, loss_pct: f64) -> f64 {
        let est = MosEstimator::new(CodecType::G711Ulaw);
        est.mos_score(rtt_ms, jitter_ms, loss_pct)
    }

    // Helper: compute full quality for any codec.
    fn quality_for(codec: CodecType, rtt_ms: f64, jitter_ms: f64, loss_pct: f64) -> CallQuality {
        let est = MosEstimator::new(codec);
        let metrics = RtpMetrics {
            rtt_ms,
            delay_ms: rtt_ms / 2.0,
            jitter_ms,
            packet_loss_pct: loss_pct,
            codec,
            packets_received: 10000,
            packets_lost: (10000.0 * loss_pct / (100.0 - loss_pct)).round() as u64,
        };
        est.estimate(&metrics)
    }

    // -----------------------------------------------------------------------
    // Perfect conditions
    // -----------------------------------------------------------------------

    #[test]
    fn test_perfect_conditions_g711() {
        // 0% loss, 20ms one-way delay (40ms RTT), 0ms jitter
        let mos = g711_mos(40.0, 0.0, 0.0);
        // Under perfect conditions with minimal delay, G.711 should achieve ~4.4
        assert!(
            mos > 4.3 && mos <= 4.5,
            "Perfect G.711 MOS should be ~4.4, got {:.3}",
            mos
        );
    }

    #[test]
    fn test_zero_everything() {
        // Absolute zero delay, zero jitter, zero loss
        let mos = g711_mos(0.0, 0.0, 0.0);
        // This gives maximum R-factor, capped at MOS 4.5
        assert!(
            mos > 4.3 && mos <= 4.5,
            "Zero-impairment MOS should be ~4.4, got {:.3}",
            mos
        );
    }

    // -----------------------------------------------------------------------
    // Packet loss impact
    // -----------------------------------------------------------------------

    #[test]
    fn test_1pct_loss() {
        // With 20ms one-way delay + 10ms jitter buffer = 30ms total:
        // Ie_eff = 95 * 1/(1+25.1) = 3.64, R ~ 89.0 -> MOS ~ 4.31
        let mos = g711_mos(40.0, 5.0, 1.0);
        assert!(
            mos > 4.2 && mos < 4.4,
            "1% loss MOS should be ~4.31, got {:.3}",
            mos
        );
    }

    #[test]
    fn test_5pct_loss() {
        // Ie_eff = 95 * 5/30.1 = 15.78, R ~ 76.9 -> MOS ~ 3.90
        let mos = g711_mos(40.0, 5.0, 5.0);
        assert!(
            mos > 3.7 && mos < 4.1,
            "5% loss MOS should be ~3.90, got {:.3}",
            mos
        );
    }

    #[test]
    fn test_10pct_loss() {
        // Ie_eff = 95 * 10/35.1 = 27.07, R ~ 65.6 -> MOS ~ 3.38
        let mos = g711_mos(40.0, 5.0, 10.0);
        assert!(
            mos > 3.1 && mos < 3.6,
            "10% loss MOS should be ~3.38, got {:.3}",
            mos
        );
    }

    #[test]
    fn test_loss_monotonically_decreases_mos() {
        let mos_0 = g711_mos(40.0, 5.0, 0.0);
        let mos_1 = g711_mos(40.0, 5.0, 1.0);
        let mos_5 = g711_mos(40.0, 5.0, 5.0);
        let mos_10 = g711_mos(40.0, 5.0, 10.0);
        let mos_20 = g711_mos(40.0, 5.0, 20.0);

        assert!(mos_0 > mos_1, "0% > 1% loss");
        assert!(mos_1 > mos_5, "1% > 5% loss");
        assert!(mos_5 > mos_10, "5% > 10% loss");
        assert!(mos_10 > mos_20, "10% > 20% loss");
    }

    // -----------------------------------------------------------------------
    // Delay impact
    // -----------------------------------------------------------------------

    #[test]
    fn test_high_delay_300ms_rtt() {
        // 300ms RTT = 150ms one-way, d = 150+10 = 160ms (below 177.3 threshold)
        // Id = 0.024*160 = 3.84, R = 89.52 -> MOS ~ 4.33
        // Significant degradation only happens above the 177.3ms threshold.
        let mos = g711_mos(300.0, 5.0, 0.0);
        assert!(
            mos < 4.4,
            "300ms RTT should degrade MOS below 4.4, got {:.3}",
            mos
        );
        // Must be lower than perfect (40ms RTT)
        let mos_perfect = g711_mos(40.0, 5.0, 0.0);
        assert!(
            mos < mos_perfect,
            "300ms RTT MOS ({:.3}) should be lower than 40ms RTT ({:.3})",
            mos,
            mos_perfect,
        );
    }

    #[test]
    fn test_very_high_delay_600ms_rtt() {
        // 600ms RTT = 300ms one-way
        let mos = g711_mos(600.0, 5.0, 0.0);
        assert!(
            mos < 4.0,
            "600ms RTT should degrade MOS below 4.0, got {:.3}",
            mos
        );
    }

    #[test]
    fn test_delay_threshold_177ms() {
        // Below 177.3ms total delay, impairment is linear (small).
        // Above 177.3ms, there's an additional penalty.
        let mos_low = g711_mos(200.0, 0.0, 0.0); // 100ms one-way
        let mos_high = g711_mos(400.0, 0.0, 0.0); // 200ms one-way (above threshold)

        assert!(
            mos_low > mos_high,
            "Higher delay should produce lower MOS"
        );
    }

    #[test]
    fn test_delay_monotonically_decreases_mos() {
        let mos_20 = g711_mos(40.0, 0.0, 0.0);
        let mos_100 = g711_mos(200.0, 0.0, 0.0);
        let mos_200 = g711_mos(400.0, 0.0, 0.0);
        let mos_500 = g711_mos(1000.0, 0.0, 0.0);

        assert!(mos_20 > mos_100, "20ms > 100ms delay");
        assert!(mos_100 > mos_200, "100ms > 200ms delay");
        assert!(mos_200 > mos_500, "200ms > 500ms delay");
    }

    // -----------------------------------------------------------------------
    // Jitter impact
    // -----------------------------------------------------------------------

    #[test]
    fn test_high_jitter_100ms() {
        let mos = g711_mos(40.0, 100.0, 0.0);
        // 100ms jitter adds ~200ms jitter buffer delay
        assert!(
            mos < 4.3,
            "100ms jitter should degrade MOS, got {:.3}",
            mos
        );
    }

    #[test]
    fn test_jitter_monotonically_decreases_mos() {
        let mos_0 = g711_mos(40.0, 0.0, 0.0);
        let mos_10 = g711_mos(40.0, 10.0, 0.0);
        let mos_50 = g711_mos(40.0, 50.0, 0.0);
        let mos_100 = g711_mos(40.0, 100.0, 0.0);

        assert!(mos_0 > mos_10, "0ms > 10ms jitter");
        assert!(mos_10 > mos_50, "10ms > 50ms jitter");
        assert!(mos_50 > mos_100, "50ms > 100ms jitter");
    }

    // -----------------------------------------------------------------------
    // Codec differences
    // -----------------------------------------------------------------------

    #[test]
    fn test_g729_lower_baseline_than_g711() {
        // G.729 has Ie=11, so even with perfect network, MOS is lower than G.711
        let mos_g711 = g711_mos(40.0, 5.0, 0.0);
        let q_g729 = quality_for(CodecType::G729, 40.0, 5.0, 0.0);

        assert!(
            mos_g711 > q_g729.mos,
            "G.711 ({:.3}) should have higher MOS than G.729 ({:.3}) at 0% loss",
            mos_g711,
            q_g729.mos
        );
    }

    #[test]
    fn test_gsm_lower_baseline_than_g711() {
        let mos_g711 = g711_mos(40.0, 5.0, 0.0);
        let q_gsm = quality_for(CodecType::GSM, 40.0, 5.0, 0.0);

        assert!(
            mos_g711 > q_gsm.mos,
            "G.711 ({:.3}) should have higher MOS than GSM ({:.3})",
            mos_g711,
            q_gsm.mos
        );
    }

    #[test]
    fn test_gsm_worse_than_g729() {
        // GSM has Ie=20 vs G.729 Ie=11
        let q_g729 = quality_for(CodecType::G729, 40.0, 5.0, 0.0);
        let q_gsm = quality_for(CodecType::GSM, 40.0, 5.0, 0.0);

        assert!(
            q_g729.mos > q_gsm.mos,
            "G.729 ({:.3}) should have higher MOS than GSM ({:.3})",
            q_g729.mos,
            q_gsm.mos
        );
    }

    #[test]
    fn test_all_codecs_produce_valid_mos_range() {
        let codecs = [
            CodecType::G711Ulaw,
            CodecType::G711Alaw,
            CodecType::G729,
            CodecType::G722,
            CodecType::Opus,
            CodecType::GSM,
            CodecType::ILBC,
            CodecType::Speex,
            CodecType::Unknown,
        ];

        for codec in &codecs {
            // Perfect conditions
            let q_perfect = quality_for(*codec, 40.0, 5.0, 0.0);
            assert!(
                q_perfect.mos >= 1.0 && q_perfect.mos <= 4.5,
                "{:?} perfect MOS {:.3} out of range [1.0, 4.5]",
                codec,
                q_perfect.mos
            );

            // Terrible conditions
            let q_terrible = quality_for(*codec, 1000.0, 200.0, 50.0);
            assert!(
                q_terrible.mos >= 1.0 && q_terrible.mos <= 4.5,
                "{:?} terrible MOS {:.3} out of range [1.0, 4.5]",
                codec,
                q_terrible.mos
            );
        }
    }

    // -----------------------------------------------------------------------
    // Quality rating thresholds
    // -----------------------------------------------------------------------

    #[test]
    fn test_quality_rating_excellent() {
        assert_eq!(QualityRating::from_r_factor(95.0), QualityRating::Excellent);
        assert_eq!(QualityRating::from_r_factor(90.0), QualityRating::Excellent);
    }

    #[test]
    fn test_quality_rating_good() {
        assert_eq!(QualityRating::from_r_factor(89.9), QualityRating::Good);
        assert_eq!(QualityRating::from_r_factor(80.0), QualityRating::Good);
    }

    #[test]
    fn test_quality_rating_fair() {
        assert_eq!(QualityRating::from_r_factor(79.9), QualityRating::Fair);
        assert_eq!(QualityRating::from_r_factor(70.0), QualityRating::Fair);
    }

    #[test]
    fn test_quality_rating_poor() {
        assert_eq!(QualityRating::from_r_factor(69.9), QualityRating::Poor);
        assert_eq!(QualityRating::from_r_factor(60.0), QualityRating::Poor);
    }

    #[test]
    fn test_quality_rating_bad() {
        assert_eq!(QualityRating::from_r_factor(59.9), QualityRating::Bad);
        assert_eq!(QualityRating::from_r_factor(0.0), QualityRating::Bad);
    }

    #[test]
    fn test_quality_rating_matches_mos() {
        // Excellent: MOS >= 4.3
        let q = quality_for(CodecType::G711Ulaw, 40.0, 0.0, 0.0);
        if q.mos >= 4.3 {
            assert_eq!(q.quality, QualityRating::Excellent);
        }

        // Bad: heavy loss
        let q = quality_for(CodecType::G711Ulaw, 40.0, 5.0, 30.0);
        assert_eq!(q.quality, QualityRating::Bad);
    }

    // -----------------------------------------------------------------------
    // Edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_zero_loss() {
        let mos = g711_mos(40.0, 5.0, 0.0);
        assert!(mos > 4.0, "Zero loss should give good MOS, got {:.3}", mos);
    }

    #[test]
    fn test_100pct_loss() {
        let mos = g711_mos(40.0, 5.0, 100.0);
        assert!(
            mos >= 1.0 && mos <= 1.5,
            "100% loss MOS should be near 1.0, got {:.3}",
            mos
        );
    }

    #[test]
    fn test_zero_delay() {
        let mos = g711_mos(0.0, 0.0, 0.0);
        assert!(
            mos > 4.3,
            "Zero delay should give excellent MOS, got {:.3}",
            mos
        );
    }

    #[test]
    fn test_extreme_delay_1000ms() {
        let mos = g711_mos(2000.0, 0.0, 0.0);
        // 1000ms one-way delay should severely degrade quality
        assert!(
            mos >= 1.0 && mos <= 4.5,
            "Extreme delay MOS {:.3} out of range",
            mos
        );
        assert!(mos < 3.5, "1000ms one-way delay should give poor MOS, got {:.3}", mos);
    }

    #[test]
    fn test_combined_impairments() {
        // All impairments together should compound
        let mos_perfect = g711_mos(40.0, 5.0, 0.0);
        let mos_loss_only = g711_mos(40.0, 5.0, 5.0);
        let mos_delay_only = g711_mos(600.0, 5.0, 0.0);
        let mos_combined = g711_mos(600.0, 5.0, 5.0);

        assert!(mos_combined < mos_loss_only, "Combined should be worse than loss only");
        assert!(mos_combined < mos_delay_only, "Combined should be worse than delay only");
        assert!(mos_combined < mos_perfect, "Combined should be worse than perfect");
    }

    // -----------------------------------------------------------------------
    // R-factor to MOS conversion
    // -----------------------------------------------------------------------

    #[test]
    fn test_r_factor_to_mos_boundaries() {
        assert_eq!(r_factor_to_mos(-10.0), 1.0);
        assert_eq!(r_factor_to_mos(0.0), 1.0);
        assert_eq!(r_factor_to_mos(110.0), 4.5);
    }

    #[test]
    fn test_r_factor_to_mos_monotonic() {
        // With clamping to [1.0, 4.5], the function should be monotonically
        // non-decreasing across the full R range.
        let mut prev_mos = 0.0;
        // Test at fine granularity (0.1 steps)
        for i in 0..=1000 {
            let r = i as f64 / 10.0;
            let mos = r_factor_to_mos(r);
            assert!(
                mos >= prev_mos - 1e-10,
                "MOS should be non-decreasing with R: R={:.1} MOS={:.6} < prev={:.6}",
                r,
                mos,
                prev_mos
            );
            prev_mos = mos;
        }
    }

    #[test]
    fn test_r_factor_to_mos_known_values() {
        // R=0 -> MOS=1.0
        assert!((r_factor_to_mos(0.0) - 1.0).abs() < 0.01);

        // R=50 -> approximately 2.6
        let mos_50 = r_factor_to_mos(50.0);
        assert!(
            mos_50 > 2.4 && mos_50 < 2.8,
            "R=50 should give MOS ~2.6, got {:.3}",
            mos_50
        );

        // R=100 -> MOS ~4.5
        let mos_100 = r_factor_to_mos(100.0);
        assert!(
            mos_100 > 4.4 && mos_100 <= 4.5,
            "R=100 should give MOS ~4.5, got {:.3}",
            mos_100
        );
    }

    // -----------------------------------------------------------------------
    // MOS to R-factor (inverse)
    // -----------------------------------------------------------------------

    #[test]
    fn test_mos_to_r_factor_roundtrip() {
        // Skip low R values where MOS is clamped to 1.0 (non-invertible).
        // The polynomial dips below 1.0 for R in roughly [0, 6.5], so
        // start from R=10 where the mapping is strictly increasing.
        for r in (10..=100).step_by(5) {
            let mos = r_factor_to_mos(r as f64);
            let r_back = mos_to_r_factor(mos);
            assert!(
                (r_back - r as f64).abs() < 0.1,
                "Roundtrip failed: R={} -> MOS={:.3} -> R={:.1}",
                r,
                mos,
                r_back
            );
        }
    }

    // -----------------------------------------------------------------------
    // RtpMetrics construction
    // -----------------------------------------------------------------------

    #[test]
    fn test_rtp_metrics_from_stats() {
        let m = RtpMetrics::from_stats(100.0, 10.0, 990, 10, CodecType::G711Ulaw);
        assert_eq!(m.delay_ms, 50.0);
        assert!((m.packet_loss_pct - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_rtp_metrics_from_stats_zero_packets() {
        let m = RtpMetrics::from_stats(100.0, 10.0, 0, 0, CodecType::G711Ulaw);
        assert_eq!(m.packet_loss_pct, 0.0);
    }

    // -----------------------------------------------------------------------
    // MosEstimator lifecycle
    // -----------------------------------------------------------------------

    #[test]
    fn test_set_codec_changes_impairment() {
        let mut est = MosEstimator::new(CodecType::G711Ulaw);
        let mos_g711 = est.mos_score(40.0, 5.0, 0.0);

        est.set_codec(CodecType::G729);
        let mos_g729 = est.mos_score(40.0, 5.0, 0.0);

        assert!(
            mos_g711 > mos_g729,
            "G.711 ({:.3}) should beat G.729 ({:.3}) after set_codec",
            mos_g711,
            mos_g729
        );
        assert_eq!(est.codec(), CodecType::G729);
    }

    // -----------------------------------------------------------------------
    // Display / formatting
    // -----------------------------------------------------------------------

    #[test]
    fn test_call_quality_display() {
        let q = quality_for(CodecType::G711Ulaw, 40.0, 5.0, 1.0);
        let s = format!("{}", q);
        assert!(s.contains("MOS="), "Display should contain MOS");
        assert!(s.contains("R="), "Display should contain R-factor");
    }

    #[test]
    fn test_quality_rating_display() {
        assert_eq!(format!("{}", QualityRating::Excellent), "Excellent");
        assert_eq!(format!("{}", QualityRating::Bad), "Bad");
    }

    #[test]
    fn test_codec_type_display() {
        assert_eq!(format!("{}", CodecType::G711Ulaw), "G.711 u-law");
        assert_eq!(format!("{}", CodecType::Opus), "Opus");
    }

    // -----------------------------------------------------------------------
    // E-model verification against known reference values
    // -----------------------------------------------------------------------

    #[test]
    fn test_emodel_reference_g711_no_impairment() {
        // G.711 with zero impairment: R = 94.768 - 1.41 - 0 - 0 + 0 = 93.358
        let est = MosEstimator::new(CodecType::G711Ulaw);
        let r = est.compute_r_factor(0.0, 0.0, 0.0);
        assert!(
            (r - 93.358).abs() < 0.01,
            "G.711 zero-impairment R should be ~93.358, got {:.3}",
            r
        );
    }

    #[test]
    fn test_emodel_reference_g729_no_impairment() {
        // G.729 with zero impairment: R = 94.768 - 1.41 - 0 - 11 + 0 = 82.358
        let est = MosEstimator::new(CodecType::G729);
        let r = est.compute_r_factor(0.0, 0.0, 0.0);
        assert!(
            (r - 82.358).abs() < 0.01,
            "G.729 zero-impairment R should be ~82.358, got {:.3}",
            r
        );
    }

    #[test]
    fn test_emodel_delay_impairment_below_threshold() {
        // d=100ms: Id = 0.024 * 100 = 2.4 (below 177.3 threshold)
        let est = MosEstimator::new(CodecType::G711Ulaw);
        let r = est.compute_r_factor(100.0, 0.0, 0.0);
        // R = 93.358 - 2.4 = 90.958
        assert!(
            (r - 90.958).abs() < 0.01,
            "100ms delay R should be ~90.958, got {:.3}",
            r
        );
    }

    #[test]
    fn test_emodel_delay_impairment_above_threshold() {
        // d=200ms: Id = 0.024 * 200 + 0.11 * (200 - 177.3) * 1 = 4.8 + 2.497 = 7.297
        let est = MosEstimator::new(CodecType::G711Ulaw);
        let r = est.compute_r_factor(200.0, 0.0, 0.0);
        // R = 93.358 - 7.297 = 86.061
        assert!(
            (r - 86.061).abs() < 0.01,
            "200ms delay R should be ~86.061, got {:.3}",
            r
        );
    }

    #[test]
    fn test_emodel_equipment_impairment() {
        // G.711 at 5% loss: Ie_eff = 0 + (95 - 0) * 5 / (5 + 25.1) = 95 * 5 / 30.1 = 15.78
        let est = MosEstimator::new(CodecType::G711Ulaw);
        let ie_eff = est.compute_equipment_impairment(5.0);
        assert!(
            (ie_eff - 15.78).abs() < 0.1,
            "G.711 5% loss Ie_eff should be ~15.78, got {:.3}",
            ie_eff
        );
    }

    // ===================================================================
    // ADVERSARIAL TESTS
    // ===================================================================

    // -------------------------------------------------------------------
    // 1. E-Model Math Verification
    // -------------------------------------------------------------------

    #[test]
    fn test_adversarial_ro_value_produces_correct_baseline() {
        // Ro=94.768, Is=1.41 per ITU-T G.107. At zero impairment for G.711:
        // R = 94.768 - 1.41 = 93.358
        let est = MosEstimator::new(CodecType::G711Ulaw);
        let r = est.compute_r_factor(0.0, 0.0, 0.0);
        assert!(
            (r - 93.358).abs() < 0.001,
            "Ro-Is must be exactly 93.358, got {:.6}",
            r
        );
    }

    #[test]
    fn test_adversarial_delay_formula_at_threshold_boundary() {
        // At d=177.3ms exactly, excess=0, H(0)=0, so Id = 0.024*177.3 = 4.2552
        let est = MosEstimator::new(CodecType::G711Ulaw);
        // one_way_delay=177.3, jitter=0 -> d=177.3
        let r_at = est.compute_r_factor(177.3, 0.0, 0.0);
        let expected_r = 93.358 - 0.024 * 177.3;
        assert!(
            (r_at - expected_r).abs() < 0.01,
            "At d=177.3 R should be {:.3}, got {:.3}",
            expected_r,
            r_at
        );

        // At d=177.4ms, excess=0.1, H=1, Id = 0.024*177.4 + 0.11*0.1 = 4.2576 + 0.011
        let r_above = est.compute_r_factor(177.4, 0.0, 0.0);
        assert!(
            r_above < r_at,
            "d=177.4 (R={:.6}) should be worse than d=177.3 (R={:.6})",
            r_above,
            r_at
        );
    }

    #[test]
    fn test_adversarial_ie_eff_approaches_95_as_loss_increases() {
        // Ie_eff = Ie + (95 - Ie) * Ppl / (Ppl + Bpl)
        // For G.711 (Ie=0, Bpl=25.1):
        //   At Ppl=100: Ie_eff = 95 * 100/125.1 = 75.94
        //   The formula asymptotically approaches 95 but never reaches it.
        let est = MosEstimator::new(CodecType::G711Ulaw);
        let ie_100 = est.compute_equipment_impairment(100.0);
        let expected = 95.0 * 100.0 / 125.1;
        assert!(
            (ie_100 - expected).abs() < 0.01,
            "Ie_eff at 100% loss should be {:.3}, got {:.3}",
            expected,
            ie_100
        );
        assert!(ie_100 < 95.0, "Ie_eff must be < 95 at finite loss");
    }

    #[test]
    fn test_adversarial_perfect_g711_mos_is_441() {
        // G.711 at 0% loss, 20ms one-way delay, 0 jitter:
        // d=20, Id=0.024*20=0.48, Ie_eff=0
        // R = 93.358 - 0.48 = 92.878
        // MOS = 1 + 0.035*92.878 + 92.878*(92.878-60)*(100-92.878)*7e-6
        //     = 1 + 3.2507 + 92.878*32.878*7.122*7e-6
        //     = 1 + 3.2507 + 0.1524 = 4.403
        let est = MosEstimator::new(CodecType::G711Ulaw);
        // RTT=40ms -> one_way=20ms
        let mos = est.mos_score(40.0, 0.0, 0.0);
        assert!(
            (mos - 4.403).abs() < 0.02,
            "Perfect G.711 MOS should be ~4.403, got {:.4}",
            mos
        );
    }

    // -------------------------------------------------------------------
    // 2. Edge Cases: NaN, Infinity, Negative
    // -------------------------------------------------------------------

    #[test]
    fn test_adversarial_nan_rtt() {
        let mos = g711_mos(f64::NAN, 0.0, 0.0);
        assert!(
            mos.is_finite() && mos >= 1.0 && mos <= 4.5,
            "NaN RTT must produce valid MOS, got {}",
            mos
        );
    }

    #[test]
    fn test_adversarial_nan_jitter() {
        let mos = g711_mos(40.0, f64::NAN, 0.0);
        assert!(
            mos.is_finite() && mos >= 1.0 && mos <= 4.5,
            "NaN jitter must produce valid MOS, got {}",
            mos
        );
    }

    #[test]
    fn test_adversarial_nan_loss() {
        let mos = g711_mos(40.0, 0.0, f64::NAN);
        assert!(
            mos.is_finite() && mos >= 1.0 && mos <= 4.5,
            "NaN loss must produce valid MOS, got {}",
            mos
        );
    }

    #[test]
    fn test_adversarial_all_nan() {
        let mos = g711_mos(f64::NAN, f64::NAN, f64::NAN);
        assert!(
            mos.is_finite() && mos >= 1.0 && mos <= 4.5,
            "All-NaN inputs must produce valid MOS, got {}",
            mos
        );
    }

    #[test]
    fn test_adversarial_infinity_rtt() {
        let mos = g711_mos(f64::INFINITY, 0.0, 0.0);
        assert!(
            mos.is_finite() && mos >= 1.0 && mos <= 4.5,
            "Inf RTT must produce valid MOS, got {}",
            mos
        );
    }

    #[test]
    fn test_adversarial_infinity_jitter() {
        let mos = g711_mos(40.0, f64::INFINITY, 0.0);
        assert!(
            mos.is_finite() && mos >= 1.0 && mos <= 4.5,
            "Inf jitter must produce valid MOS, got {}",
            mos
        );
    }

    #[test]
    fn test_adversarial_infinity_loss() {
        let mos = g711_mos(40.0, 0.0, f64::INFINITY);
        assert!(
            mos.is_finite() && mos >= 1.0 && mos <= 4.5,
            "Inf loss must produce valid MOS, got {}",
            mos
        );
    }

    #[test]
    fn test_adversarial_neg_infinity_all() {
        let mos = g711_mos(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        assert!(
            mos.is_finite() && mos >= 1.0 && mos <= 4.5,
            "-Inf inputs must produce valid MOS, got {}",
            mos
        );
    }

    #[test]
    fn test_adversarial_negative_rtt() {
        let mos = g711_mos(-100.0, 0.0, 0.0);
        assert!(
            mos.is_finite() && mos >= 1.0 && mos <= 4.5,
            "Negative RTT must produce valid MOS, got {}",
            mos
        );
        // Negative RTT should be clamped to 0, giving same result as 0.
        let mos_zero = g711_mos(0.0, 0.0, 0.0);
        assert!(
            (mos - mos_zero).abs() < 1e-10,
            "Negative RTT ({:.6}) should equal zero RTT ({:.6})",
            mos,
            mos_zero
        );
    }

    #[test]
    fn test_adversarial_negative_jitter() {
        let mos = g711_mos(40.0, -50.0, 0.0);
        assert!(
            mos.is_finite() && mos >= 1.0 && mos <= 4.5,
            "Negative jitter must produce valid MOS, got {}",
            mos
        );
        // Should be same as jitter=0
        let mos_zero_j = g711_mos(40.0, 0.0, 0.0);
        assert!(
            (mos - mos_zero_j).abs() < 1e-10,
            "Negative jitter ({:.6}) should equal zero jitter ({:.6})",
            mos,
            mos_zero_j
        );
    }

    #[test]
    fn test_adversarial_negative_loss() {
        let mos = g711_mos(40.0, 0.0, -10.0);
        assert!(
            mos.is_finite() && mos >= 1.0 && mos <= 4.5,
            "Negative loss must produce valid MOS, got {}",
            mos
        );
        // Should be same as loss=0
        let mos_zero_l = g711_mos(40.0, 0.0, 0.0);
        assert!(
            (mos - mos_zero_l).abs() < 1e-10,
            "Negative loss ({:.6}) should equal zero loss ({:.6})",
            mos,
            mos_zero_l
        );
    }

    #[test]
    fn test_adversarial_all_negative() {
        let mos = g711_mos(-10.0, -5.0, -1.0);
        assert!(
            mos.is_finite() && mos >= 1.0 && mos <= 4.5,
            "All-negative inputs must produce valid MOS, got {}",
            mos
        );
    }

    #[test]
    fn test_adversarial_loss_over_100() {
        let mos = g711_mos(40.0, 0.0, 200.0);
        assert!(
            mos.is_finite() && mos >= 1.0 && mos <= 4.5,
            "Loss > 100% must produce valid MOS, got {}",
            mos
        );
        // Should be same as loss=100
        let mos_100 = g711_mos(40.0, 0.0, 100.0);
        assert!(
            (mos - mos_100).abs() < 1e-10,
            "Loss 200% ({:.6}) should equal loss 100% ({:.6})",
            mos,
            mos_100
        );
    }

    #[test]
    fn test_adversarial_loss_exactly_0() {
        let est = MosEstimator::new(CodecType::G711Ulaw);
        let ie = est.compute_equipment_impairment(0.0);
        assert!(
            ie.abs() < 1e-10,
            "Ie_eff at 0% loss for G.711 should be 0, got {}",
            ie
        );
    }

    #[test]
    fn test_adversarial_loss_exactly_100() {
        let est = MosEstimator::new(CodecType::G711Ulaw);
        let ie = est.compute_equipment_impairment(100.0);
        // Ie_eff = 95 * 100 / 125.1 = 75.94
        assert!(
            (ie - 75.94).abs() < 0.01,
            "Ie_eff at 100% loss should be ~75.94, got {:.3}",
            ie
        );
    }

    #[test]
    fn test_adversarial_very_high_delay_10000ms() {
        let mos = g711_mos(20000.0, 0.0, 0.0);
        assert!(
            mos.is_finite() && mos >= 1.0 && mos <= 4.5,
            "10000ms delay must produce valid MOS, got {}",
            mos
        );
        assert_eq!(mos, 1.0, "10000ms delay should produce MOS=1.0");
    }

    #[test]
    fn test_adversarial_very_high_jitter_5000ms() {
        let mos = g711_mos(40.0, 5000.0, 0.0);
        assert!(
            mos.is_finite() && mos >= 1.0 && mos <= 4.5,
            "5000ms jitter must produce valid MOS, got {}",
            mos
        );
        assert_eq!(mos, 1.0, "5000ms jitter should produce MOS=1.0");
    }

    // -------------------------------------------------------------------
    // 3. Codec Impairment Value Verification (ITU-T G.113 Appendix I)
    // -------------------------------------------------------------------

    #[test]
    fn test_adversarial_g711_impairment_values() {
        // G.113: G.711 Ie=0, Bpl=25.1
        assert_eq!(CodecType::G711Ulaw.ie(), 0.0);
        assert_eq!(CodecType::G711Ulaw.bpl(), 25.1);
        assert_eq!(CodecType::G711Alaw.ie(), 0.0);
        assert_eq!(CodecType::G711Alaw.bpl(), 25.1);
    }

    #[test]
    fn test_adversarial_g729_impairment_values() {
        // G.113: G.729 Ie=11, Bpl=19
        assert_eq!(CodecType::G729.ie(), 11.0);
        assert_eq!(CodecType::G729.bpl(), 19.0);
    }

    #[test]
    fn test_adversarial_gsm_impairment_values() {
        // G.113: GSM FR Ie=20 (conservative; some sources cite 23 for
        // half-rate). Bpl=17 is standard for GSM FR.
        assert_eq!(CodecType::GSM.ie(), 20.0);
        assert_eq!(CodecType::GSM.bpl(), 17.0);
    }

    // -------------------------------------------------------------------
    // 4. MOS Range: always in [1.0, 4.5]
    // -------------------------------------------------------------------

    #[test]
    fn test_adversarial_mos_range_exhaustive() {
        // Test a grid of 1000+ combinations across all codecs.
        let codecs = [
            CodecType::G711Ulaw,
            CodecType::G729,
            CodecType::GSM,
            CodecType::Unknown,
        ];
        let rtts = [0.0, 10.0, 40.0, 100.0, 300.0, 600.0, 2000.0, 10000.0];
        let jitters = [0.0, 5.0, 20.0, 100.0, 500.0, 5000.0];
        let losses = [0.0, 0.1, 1.0, 5.0, 10.0, 25.0, 50.0, 75.0, 100.0];

        let mut count = 0;
        for codec in &codecs {
            let est = MosEstimator::new(*codec);
            for &rtt in &rtts {
                for &jit in &jitters {
                    for &loss in &losses {
                        let mos = est.mos_score(rtt, jit, loss);
                        assert!(
                            mos.is_finite() && mos >= 1.0 && mos <= 4.5,
                            "MOS out of range for {:?} rtt={} jit={} loss={}: {}",
                            codec,
                            rtt,
                            jit,
                            loss,
                            mos
                        );
                        count += 1;
                    }
                }
            }
        }
        assert!(
            count >= 1000,
            "Must test at least 1000 combinations, tested {}",
            count
        );
    }

    #[test]
    fn test_adversarial_r_factor_always_in_range() {
        let est = MosEstimator::new(CodecType::G711Ulaw);
        for rtt in (0..=2000).step_by(50) {
            for loss in (0..=100).step_by(5) {
                let r = est.compute_r_factor(
                    rtt as f64 / 2.0,
                    10.0,
                    loss as f64,
                );
                assert!(
                    r.is_finite() && r >= 0.0 && r <= 100.0,
                    "R out of [0,100] for delay={} loss={}: {}",
                    rtt / 2,
                    loss,
                    r
                );
            }
        }
    }

    // -------------------------------------------------------------------
    // 5. Quality Rating Boundary Tests
    // -------------------------------------------------------------------

    #[test]
    fn test_adversarial_quality_boundary_r90() {
        // R=90.0 exactly: >= 90 -> Excellent
        assert_eq!(QualityRating::from_r_factor(90.0), QualityRating::Excellent);
        assert_eq!(QualityRating::from_r_factor(89.999), QualityRating::Good);
    }

    #[test]
    fn test_adversarial_quality_boundary_r80() {
        // R=80.0 exactly: >= 80 -> Good
        assert_eq!(QualityRating::from_r_factor(80.0), QualityRating::Good);
        assert_eq!(QualityRating::from_r_factor(79.999), QualityRating::Fair);
    }

    #[test]
    fn test_adversarial_quality_boundary_r70() {
        assert_eq!(QualityRating::from_r_factor(70.0), QualityRating::Fair);
        assert_eq!(QualityRating::from_r_factor(69.999), QualityRating::Poor);
    }

    #[test]
    fn test_adversarial_quality_boundary_r60() {
        assert_eq!(QualityRating::from_r_factor(60.0), QualityRating::Poor);
        assert_eq!(QualityRating::from_r_factor(59.999), QualityRating::Bad);
    }

    // -------------------------------------------------------------------
    // 6. Monotonicity
    // -------------------------------------------------------------------

    #[test]
    fn test_adversarial_loss_strictly_monotonic_fine_grained() {
        // MOS must strictly decrease as loss goes from 0% to 100% in 1% steps.
        let est = MosEstimator::new(CodecType::G711Ulaw);
        let mut prev_mos = est.mos_score(40.0, 5.0, 0.0);
        for loss_x10 in 1..=1000 {
            let loss = loss_x10 as f64 / 10.0;
            let mos = est.mos_score(40.0, 5.0, loss);
            assert!(
                mos < prev_mos || (mos == 1.0 && prev_mos == 1.0),
                "MOS must decrease as loss increases: loss={:.1}% mos={:.6} >= prev={:.6}",
                loss,
                mos,
                prev_mos
            );
            prev_mos = mos;
        }
    }

    #[test]
    fn test_adversarial_delay_monotonic_fine_grained() {
        // MOS must decrease (or stay at 1.0) as delay increases.
        let est = MosEstimator::new(CodecType::G711Ulaw);
        let mut prev_mos = est.mos_score(0.0, 0.0, 0.0);
        for rtt in (10..=5000).step_by(10) {
            let mos = est.mos_score(rtt as f64, 0.0, 0.0);
            assert!(
                mos <= prev_mos + 1e-10,
                "MOS must not increase with delay: rtt={}ms mos={:.6} > prev={:.6}",
                rtt,
                mos,
                prev_mos
            );
            prev_mos = mos;
        }
    }

    #[test]
    fn test_adversarial_jitter_monotonic_fine_grained() {
        // MOS must decrease (or stay at 1.0) as jitter increases.
        let est = MosEstimator::new(CodecType::G711Ulaw);
        let mut prev_mos = est.mos_score(40.0, 0.0, 0.0);
        for jit in (1..=2000).step_by(1) {
            let mos = est.mos_score(40.0, jit as f64, 0.0);
            assert!(
                mos <= prev_mos + 1e-10,
                "MOS must not increase with jitter: jit={}ms mos={:.6} > prev={:.6}",
                jit,
                mos,
                prev_mos
            );
            prev_mos = mos;
        }
    }

    // -------------------------------------------------------------------
    // 7. r_factor_to_mos edge cases
    // -------------------------------------------------------------------

    #[test]
    fn test_adversarial_r_factor_to_mos_nan() {
        let mos = r_factor_to_mos(f64::NAN);
        assert_eq!(mos, 1.0, "NaN R-factor must produce MOS=1.0");
    }

    #[test]
    fn test_adversarial_r_factor_to_mos_pos_infinity() {
        let mos = r_factor_to_mos(f64::INFINITY);
        assert_eq!(mos, 4.5, "Inf R-factor must produce MOS=4.5");
    }

    #[test]
    fn test_adversarial_r_factor_to_mos_neg_infinity() {
        let mos = r_factor_to_mos(f64::NEG_INFINITY);
        assert_eq!(mos, 1.0, "-Inf R-factor must produce MOS=1.0");
    }

    // -------------------------------------------------------------------
    // 8. mos_to_r_factor edge cases
    // -------------------------------------------------------------------

    #[test]
    fn test_adversarial_mos_to_r_nan() {
        let r = mos_to_r_factor(f64::NAN);
        assert_eq!(r, 0.0, "NaN MOS must produce R=0");
    }

    #[test]
    fn test_adversarial_mos_to_r_infinity() {
        let r = mos_to_r_factor(f64::INFINITY);
        assert_eq!(r, 100.0, "Inf MOS must produce R=100");
    }

    #[test]
    fn test_adversarial_mos_to_r_neg_infinity() {
        let r = mos_to_r_factor(f64::NEG_INFINITY);
        assert_eq!(r, 0.0, "-Inf MOS must produce R=0");
    }

    // -------------------------------------------------------------------
    // 9. estimate() with pathological RtpMetrics
    // -------------------------------------------------------------------

    #[test]
    fn test_adversarial_estimate_with_nan_metrics() {
        let est = MosEstimator::new(CodecType::G711Ulaw);
        let metrics = RtpMetrics {
            rtt_ms: f64::NAN,
            delay_ms: f64::NAN,
            jitter_ms: f64::NAN,
            packet_loss_pct: f64::NAN,
            codec: CodecType::G711Ulaw,
            packets_received: 1000,
            packets_lost: 0,
        };
        let q = est.estimate(&metrics);
        assert!(
            q.mos.is_finite() && q.mos >= 1.0 && q.mos <= 4.5,
            "estimate() with NaN metrics must produce valid MOS, got {}",
            q.mos
        );
        assert!(
            q.r_factor.is_finite() && q.r_factor >= 0.0 && q.r_factor <= 100.0,
            "estimate() with NaN metrics must produce valid R, got {}",
            q.r_factor
        );
    }

    #[test]
    fn test_adversarial_estimate_with_infinity_metrics() {
        let est = MosEstimator::new(CodecType::G711Ulaw);
        let metrics = RtpMetrics {
            rtt_ms: f64::INFINITY,
            delay_ms: f64::INFINITY,
            jitter_ms: f64::INFINITY,
            packet_loss_pct: f64::INFINITY,
            codec: CodecType::G711Ulaw,
            packets_received: 1000,
            packets_lost: 1000,
        };
        let q = est.estimate(&metrics);
        assert!(
            q.mos.is_finite() && q.mos >= 1.0 && q.mos <= 4.5,
            "estimate() with Inf metrics must produce valid MOS, got {}",
            q.mos
        );
    }

    // -------------------------------------------------------------------
    // 10. Negative impairment must NOT improve score above baseline
    // -------------------------------------------------------------------

    #[test]
    fn test_adversarial_negative_inputs_cannot_exceed_baseline() {
        // With negative inputs clamped to 0, the MOS should never exceed
        // the zero-impairment baseline.
        let baseline = g711_mos(0.0, 0.0, 0.0);
        let with_negatives = g711_mos(-1000.0, -500.0, -50.0);
        assert!(
            (with_negatives - baseline).abs() < 1e-10,
            "Negative inputs ({:.6}) must not exceed baseline ({:.6})",
            with_negatives,
            baseline
        );
    }

    // -------------------------------------------------------------------
    // 11. Sanitization helpers
    // -------------------------------------------------------------------

    #[test]
    fn test_adversarial_sanitize_non_negative() {
        assert_eq!(sanitize_non_negative(0.0), 0.0);
        assert_eq!(sanitize_non_negative(42.0), 42.0);
        assert_eq!(sanitize_non_negative(-1.0), 0.0);
        assert_eq!(sanitize_non_negative(-1e300), 0.0);
        assert_eq!(sanitize_non_negative(f64::NAN), 0.0);
        assert_eq!(sanitize_non_negative(f64::INFINITY), f64::INFINITY);
        assert_eq!(sanitize_non_negative(f64::NEG_INFINITY), 0.0);
    }

    #[test]
    fn test_adversarial_sanitize_loss() {
        assert_eq!(sanitize_loss(0.0), 0.0);
        assert_eq!(sanitize_loss(50.0), 50.0);
        assert_eq!(sanitize_loss(100.0), 100.0);
        assert_eq!(sanitize_loss(-10.0), 0.0);
        assert_eq!(sanitize_loss(200.0), 100.0);
        assert_eq!(sanitize_loss(f64::NAN), 100.0);
        assert_eq!(sanitize_loss(f64::INFINITY), 100.0);
        assert_eq!(sanitize_loss(f64::NEG_INFINITY), 0.0);
    }
}
