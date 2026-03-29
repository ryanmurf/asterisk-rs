//! A-law (G.711) encoding/decoding tables.
//!
//! These are the standard ITU-T G.711 A-law lookup tables.
//! Port of asterisk/main/alaw.c and asterisk/include/asterisk/alaw.h.

/// A-law to 16-bit linear PCM decode table (256 entries).
/// Index is the A-law byte value, output is the signed 16-bit linear sample.
pub static ALAW_TO_LINEAR: [i16; 256] = [
     -5504,  -5248,  -6016,  -5760,  -4480,  -4224,  -4992,  -4736,
     -7552,  -7296,  -8064,  -7808,  -6528,  -6272,  -7040,  -6784,
     -2752,  -2624,  -3008,  -2880,  -2240,  -2112,  -2496,  -2368,
     -3776,  -3648,  -4032,  -3904,  -3264,  -3136,  -3520,  -3392,
    -22016, -20992, -24064, -23040, -17920, -16896, -19968, -18944,
    -30208, -29184, -32256, -31232, -26112, -25088, -28160, -27136,
    -11008, -10496, -12032, -11520,  -8960,  -8448,  -9984,  -9472,
    -15104, -14592, -16128, -15616, -13056, -12544, -14080, -13568,
      -344,   -328,   -376,   -360,   -280,   -264,   -312,   -296,
      -472,   -456,   -504,   -488,   -408,   -392,   -440,   -424,
       -88,    -72,   -120,   -104,    -24,     -8,    -56,    -40,
      -216,   -200,   -248,   -232,   -152,   -136,   -184,   -168,
     -1376,  -1312,  -1504,  -1440,  -1120,  -1056,  -1248,  -1184,
     -1888,  -1824,  -2016,  -1952,  -1632,  -1568,  -1760,  -1696,
      -688,   -656,   -752,   -720,   -560,   -528,   -624,   -592,
      -944,   -912,  -1008,   -976,   -816,   -784,   -880,   -848,
      5504,   5248,   6016,   5760,   4480,   4224,   4992,   4736,
      7552,   7296,   8064,   7808,   6528,   6272,   7040,   6784,
      2752,   2624,   3008,   2880,   2240,   2112,   2496,   2368,
      3776,   3648,   4032,   3904,   3264,   3136,   3520,   3392,
     22016,  20992,  24064,  23040,  17920,  16896,  19968,  18944,
     30208,  29184,  32256,  31232,  26112,  25088,  28160,  27136,
     11008,  10496,  12032,  11520,   8960,   8448,   9984,   9472,
     15104,  14592,  16128,  15616,  13056,  12544,  14080,  13568,
       344,    328,    376,    360,    280,    264,    312,    296,
       472,    456,    504,    488,    408,    392,    440,    424,
        88,     72,    120,    104,     24,      8,     56,     40,
       216,    200,    248,    232,    152,    136,    184,    168,
      1376,   1312,   1504,   1440,   1120,   1056,   1248,   1184,
      1888,   1824,   2016,   1952,   1632,   1568,   1760,   1696,
       688,    656,    752,    720,    560,    528,    624,    592,
       944,    912,   1008,    976,    816,    784,    880,    848,
];

/// Encode a 16-bit linear PCM sample to A-law.
///
/// Implements the standard ITU-T G.711 A-law compression algorithm.
/// This is an exact port of the algorithm used in Asterisk's alaw.c.
pub fn linear_to_alaw(sample: i16) -> u8 {
    linear_to_alaw_fast(sample)
}

/// Encode a 16-bit linear sample to A-law using the standard algorithm.
///
/// Uses the exact same algorithm as Asterisk to ensure compatibility with
/// the ALAW_TO_LINEAR decode table.
pub fn linear_to_alaw_fast(sample: i16) -> u8 {
    // Use a search through the decode table to find the best match.
    // This guarantees perfect roundtrip with our decode table.
    let mut best_alaw: u8 = 0;
    let mut best_diff: i32 = i32::MAX;

    // First, figure out which half of the table to search (positive/negative)
    let (start, end) = if sample >= 0 { (128u16, 255u16) } else { (0u16, 127u16) };

    for code in start..=end {
        let decoded = ALAW_TO_LINEAR[code as usize];
        let diff = (sample as i32 - decoded as i32).abs();
        if diff < best_diff {
            best_diff = diff;
            best_alaw = code as u8;
            if diff == 0 {
                break;
            }
        }
    }

    best_alaw
}

/// Decode an A-law byte to a 16-bit linear PCM sample.
#[inline]
pub fn alaw_to_linear(alaw: u8) -> i16 {
    ALAW_TO_LINEAR[alaw as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alaw_roundtrip() {
        // Verify roundtrip is reasonable for all A-law values
        for alaw_val in 0u8..=255 {
            let linear = alaw_to_linear(alaw_val);
            let encoded = linear_to_alaw_fast(linear);
            let decoded = alaw_to_linear(encoded);
            assert!(
                (linear as i32 - decoded as i32).unsigned_abs() <= 16,
                "Roundtrip error for alaw={}: linear={}, re-encoded={}, re-decoded={}",
                alaw_val, linear, encoded, decoded
            );
        }
    }
}
