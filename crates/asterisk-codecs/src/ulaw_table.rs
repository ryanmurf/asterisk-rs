//! mu-law (G.711) encoding/decoding tables.
//!
//! These are the standard ITU-T G.711 mu-law lookup tables used in Asterisk.
//! Port of asterisk/main/ulaw.c and asterisk/include/asterisk/ulaw.h.

/// mu-law to 16-bit linear PCM decode table (256 entries).
/// Index is the mu-law byte value, output is the signed 16-bit linear sample.
pub static MULAW_TO_LINEAR: [i16; 256] = [
    -32124, -31100, -30076, -29052, -28028, -27004, -25980, -24956,
    -23932, -22908, -21884, -20860, -19836, -18812, -17788, -16764,
    -15996, -15484, -14972, -14460, -13948, -13436, -12924, -12412,
    -11900, -11388, -10876, -10364,  -9852,  -9340,  -8828,  -8316,
     -7932,  -7676,  -7420,  -7164,  -6908,  -6652,  -6396,  -6140,
     -5884,  -5628,  -5372,  -5116,  -4860,  -4604,  -4348,  -4092,
     -3900,  -3772,  -3644,  -3516,  -3388,  -3260,  -3132,  -3004,
     -2876,  -2748,  -2620,  -2492,  -2364,  -2236,  -2108,  -1980,
     -1884,  -1820,  -1756,  -1692,  -1628,  -1564,  -1500,  -1436,
     -1372,  -1308,  -1244,  -1180,  -1116,  -1052,   -988,   -924,
      -876,   -844,   -812,   -780,   -748,   -716,   -684,   -652,
      -620,   -588,   -556,   -524,   -492,   -460,   -428,   -396,
      -372,   -356,   -340,   -324,   -308,   -292,   -276,   -260,
      -244,   -228,   -212,   -196,   -180,   -164,   -148,   -132,
      -120,   -112,   -104,    -96,    -88,    -80,    -72,    -64,
       -56,    -48,    -40,    -32,    -24,    -16,     -8,      0,
     32124,  31100,  30076,  29052,  28028,  27004,  25980,  24956,
     23932,  22908,  21884,  20860,  19836,  18812,  17788,  16764,
     15996,  15484,  14972,  14460,  13948,  13436,  12924,  12412,
     11900,  11388,  10876,  10364,   9852,   9340,   8828,   8316,
      7932,   7676,   7420,   7164,   6908,   6652,   6396,   6140,
      5884,   5628,   5372,   5116,   4860,   4604,   4348,   4092,
      3900,   3772,   3644,   3516,   3388,   3260,   3132,   3004,
      2876,   2748,   2620,   2492,   2364,   2236,   2108,   1980,
      1884,   1820,   1756,   1692,   1628,   1564,   1500,   1436,
      1372,   1308,   1244,   1180,   1116,   1052,    988,    924,
       876,    844,    812,    780,    748,    716,    684,    652,
       620,    588,    556,    524,    492,    460,    428,    396,
       372,    356,    340,    324,    308,    292,    276,    260,
       244,    228,    212,    196,    180,    164,    148,    132,
       120,    112,    104,     96,     88,     80,     72,     64,
        56,     48,     40,     32,     24,     16,      8,      0,
];

/// Encode a 16-bit linear PCM sample to mu-law.
///
/// This implements the standard ITU-T G.711 mu-law compression algorithm.
/// Exact port of the C `linear2ulaw()` from asterisk/main/ulaw.c.
pub fn linear_to_mulaw(sample: i16) -> u8 {
    // Exponent lookup table -- maps (biased_sample >> 7) to exponent.
    // This is an exact copy of the exp_lut[] table from asterisk/main/ulaw.c.
    #[rustfmt::skip]
    static EXP_LUT: [i32; 256] = [
        0,0,1,1,2,2,2,2,3,3,3,3,3,3,3,3,
        4,4,4,4,4,4,4,4,4,4,4,4,4,4,4,4,
        5,5,5,5,5,5,5,5,5,5,5,5,5,5,5,5,
        5,5,5,5,5,5,5,5,5,5,5,5,5,5,5,5,
        6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,
        6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,
        6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,
        6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,6,
        7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,
        7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,
        7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,
        7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,
        7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,
        7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,
        7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,
        7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,
    ];

    const BIAS: i32 = 0x84;
    const CLIP: i32 = 32635;

    let mut sample_i32 = sample as i32;

    // Get the sample into sign-magnitude
    let sign = (sample_i32 >> 8) & 0x80;
    if sign != 0 {
        sample_i32 = -sample_i32;
    }

    // Clip the magnitude
    if sample_i32 > CLIP {
        sample_i32 = CLIP;
    }

    // Convert from 16-bit linear to mu-law
    sample_i32 += BIAS;
    let exponent = EXP_LUT[((sample_i32 >> 7) & 0xFF) as usize];
    let mantissa = (sample_i32 >> (exponent + 3)) & 0x0F;

    // Combine sign, exponent, and mantissa, then complement
    !(sign | (exponent << 4) | mantissa) as u8
}

/// Decode a mu-law byte to a 16-bit linear PCM sample.
#[inline]
pub fn mulaw_to_linear(mulaw: u8) -> i16 {
    MULAW_TO_LINEAR[mulaw as usize]
}

/// Encode a 16-bit linear sample to mu-law using a fast lookup table.
///
/// This uses a search-based approach that is correct for all 16-bit input values.
pub fn linear_to_mulaw_fast(sample: i16) -> u8 {
    const BIAS: i32 = 132;
    const CLIP: i32 = 32635;

    let mut pcm_val = sample as i32;
    let sign = if pcm_val < 0 {
        pcm_val = -pcm_val;
        0x80i32
    } else {
        0i32
    };

    if pcm_val > CLIP {
        pcm_val = CLIP;
    }
    pcm_val += BIAS;

    let exponent = match pcm_val {
        0..=0xFF => 0,
        0x100..=0x1FF => 1,
        0x200..=0x3FF => 2,
        0x400..=0x7FF => 3,
        0x800..=0xFFF => 4,
        0x1000..=0x1FFF => 5,
        0x2000..=0x3FFF => 6,
        _ => 7,
    };

    let mantissa = (pcm_val >> (exponent + 3)) & 0x0F;
    let mulaw_byte = !(sign | (exponent << 4) | mantissa);
    mulaw_byte as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mulaw_roundtrip() {
        // Test that encode/decode roundtrips are reasonable
        for ulaw_val in 0u8..=255 {
            let linear = mulaw_to_linear(ulaw_val);
            let encoded = linear_to_mulaw_fast(linear);
            let decoded = mulaw_to_linear(encoded);
            // The roundtrip should be exact or very close
            assert!(
                (linear as i32 - decoded as i32).unsigned_abs() <= 4,
                "Roundtrip failed for ulaw={}: linear={}, re-encoded={}, re-decoded={}",
                ulaw_val, linear, encoded, decoded
            );
        }
    }

    #[test]
    fn test_mulaw_silence() {
        // mu-law silence is typically 0xFF (positive zero)
        let linear = mulaw_to_linear(0xFF);
        assert_eq!(linear, 0);
    }

    #[test]
    fn test_mulaw_negative_silence() {
        // 0x7F is negative zero
        let linear = mulaw_to_linear(0x7F);
        assert_eq!(linear, 0);
    }
}
