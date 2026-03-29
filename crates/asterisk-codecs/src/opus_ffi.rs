//! Opus FFI bridge with feature-gated native codec support.
//!
//! When compiled with `--features native-opus`, this module provides
//! FFI bindings to libopus for high-quality Opus encode/decode.
//!
//! When compiled without the feature, it provides stub implementations
//! that return an error indicating native Opus is not available.

use crate::translate::TranslateError;

// ---------------------------------------------------------------------------
// Native Opus FFI (feature = "native-opus")
// ---------------------------------------------------------------------------

#[cfg(feature = "native-opus")]
mod native {
    use std::os::raw::c_int;

    // Opaque FFI types
    #[repr(C)]
    pub struct OpusEncoder {
        _private: [u8; 0],
    }

    #[repr(C)]
    pub struct OpusDecoder {
        _private: [u8; 0],
    }

    // Opus application constants
    pub const OPUS_APPLICATION_VOIP: c_int = 2048;
    pub const OPUS_APPLICATION_AUDIO: c_int = 2049;
    pub const OPUS_APPLICATION_RESTRICTED_LOWDELAY: c_int = 2051;

    // Opus error codes
    pub const OPUS_OK: c_int = 0;

    extern "C" {
        pub fn opus_encoder_create(
            fs: i32,
            channels: c_int,
            application: c_int,
            error: *mut c_int,
        ) -> *mut OpusEncoder;

        pub fn opus_encode(
            enc: *mut OpusEncoder,
            pcm: *const i16,
            frame_size: c_int,
            data: *mut u8,
            max_data_bytes: i32,
        ) -> i32;

        pub fn opus_encoder_destroy(enc: *mut OpusEncoder);

        pub fn opus_decoder_create(
            fs: i32,
            channels: c_int,
            error: *mut c_int,
        ) -> *mut OpusDecoder;

        pub fn opus_decode(
            dec: *mut OpusDecoder,
            data: *const u8,
            len: i32,
            pcm: *mut i16,
            frame_size: c_int,
            decode_fec: c_int,
        ) -> i32;

        pub fn opus_decoder_destroy(dec: *mut OpusDecoder);
    }

    /// Safe wrapper around the native Opus encoder.
    pub struct SafeOpusEncoder {
        ptr: *mut OpusEncoder,
        sample_rate: i32,
        channels: i32,
    }

    // SAFETY: The Opus encoder is thread-safe for independent instances.
    unsafe impl Send for SafeOpusEncoder {}

    impl SafeOpusEncoder {
        /// Create a new encoder.
        pub fn new(
            sample_rate: i32,
            channels: i32,
            application: c_int,
        ) -> Result<Self, super::TranslateError> {
            let mut error: c_int = 0;
            let ptr = unsafe {
                opus_encoder_create(sample_rate, channels, application, &mut error)
            };
            if ptr.is_null() || error != OPUS_OK {
                return Err(super::TranslateError::Failed(format!(
                    "opus_encoder_create failed with error {}",
                    error
                )));
            }
            Ok(Self {
                ptr,
                sample_rate,
                channels,
            })
        }

        /// Encode PCM samples to Opus.
        pub fn encode(&mut self, pcm: &[i16], output: &mut [u8]) -> Result<usize, super::TranslateError> {
            let frame_size = pcm.len() as c_int / self.channels;
            let result = unsafe {
                opus_encode(
                    self.ptr,
                    pcm.as_ptr(),
                    frame_size,
                    output.as_mut_ptr(),
                    output.len() as i32,
                )
            };
            if result < 0 {
                return Err(super::TranslateError::Failed(format!(
                    "opus_encode failed with error {}",
                    result
                )));
            }
            Ok(result as usize)
        }
    }

    impl Drop for SafeOpusEncoder {
        fn drop(&mut self) {
            unsafe {
                opus_encoder_destroy(self.ptr);
            }
        }
    }

    /// Safe wrapper around the native Opus decoder.
    pub struct SafeOpusDecoder {
        ptr: *mut OpusDecoder,
        sample_rate: i32,
        channels: i32,
    }

    // SAFETY: The Opus decoder is thread-safe for independent instances.
    unsafe impl Send for SafeOpusDecoder {}

    impl SafeOpusDecoder {
        /// Create a new decoder.
        pub fn new(
            sample_rate: i32,
            channels: i32,
        ) -> Result<Self, super::TranslateError> {
            let mut error: c_int = 0;
            let ptr = unsafe { opus_decoder_create(sample_rate, channels, &mut error) };
            if ptr.is_null() || error != OPUS_OK {
                return Err(super::TranslateError::Failed(format!(
                    "opus_decoder_create failed with error {}",
                    error
                )));
            }
            Ok(Self {
                ptr,
                sample_rate,
                channels,
            })
        }

        /// Decode Opus data to PCM.
        pub fn decode(
            &mut self,
            data: &[u8],
            output: &mut [i16],
            fec: bool,
        ) -> Result<usize, super::TranslateError> {
            let frame_size = output.len() as i32 / self.channels;
            let result = unsafe {
                opus_decode(
                    self.ptr,
                    data.as_ptr(),
                    data.len() as i32,
                    output.as_mut_ptr(),
                    frame_size,
                    if fec { 1 } else { 0 },
                )
            };
            if result < 0 {
                return Err(super::TranslateError::Failed(format!(
                    "opus_decode failed with error {}",
                    result
                )));
            }
            Ok(result as usize)
        }
    }

    impl Drop for SafeOpusDecoder {
        fn drop(&mut self) {
            unsafe {
                opus_decoder_destroy(self.ptr);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Stub implementation (no native-opus feature)
// ---------------------------------------------------------------------------

#[cfg(not(feature = "native-opus"))]
mod stub {
    use super::TranslateError;

    /// Stub Opus encoder when native-opus is not enabled.
    pub struct SafeOpusEncoder;

    impl SafeOpusEncoder {
        pub fn new(
            _sample_rate: i32,
            _channels: i32,
            _application: i32,
        ) -> Result<Self, TranslateError> {
            Err(TranslateError::Failed(
                "Native Opus not available. Build with --features native-opus".into(),
            ))
        }

        pub fn encode(&mut self, _pcm: &[i16], _output: &mut [u8]) -> Result<usize, TranslateError> {
            Err(TranslateError::Failed(
                "Native Opus not available".into(),
            ))
        }
    }

    /// Stub Opus decoder when native-opus is not enabled.
    pub struct SafeOpusDecoder;

    impl SafeOpusDecoder {
        pub fn new(
            _sample_rate: i32,
            _channels: i32,
        ) -> Result<Self, TranslateError> {
            Err(TranslateError::Failed(
                "Native Opus not available. Build with --features native-opus".into(),
            ))
        }

        pub fn decode(
            &mut self,
            _data: &[u8],
            _output: &mut [i16],
            _fec: bool,
        ) -> Result<usize, TranslateError> {
            Err(TranslateError::Failed(
                "Native Opus not available".into(),
            ))
        }
    }
}

// Re-export the active implementation
#[cfg(feature = "native-opus")]
pub use native::{SafeOpusDecoder, SafeOpusEncoder};

#[cfg(not(feature = "native-opus"))]
pub use stub::{SafeOpusDecoder, SafeOpusEncoder};

/// Check at runtime whether native Opus is available.
pub fn is_native_opus_available() -> bool {
    cfg!(feature = "native-opus")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_opus_availability() {
        // Without the feature, native should not be available
        #[cfg(not(feature = "native-opus"))]
        {
            assert!(!is_native_opus_available());
            assert!(SafeOpusEncoder::new(48000, 2, 2048).is_err());
            assert!(SafeOpusDecoder::new(48000, 2).is_err());
        }

        #[cfg(feature = "native-opus")]
        {
            assert!(is_native_opus_available());
        }
    }
}
