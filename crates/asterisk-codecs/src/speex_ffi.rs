//! Speex FFI bridge with feature-gated native codec support.
//!
//! When compiled with `--features native-speex`, this module provides
//! FFI bindings to libspeex for Speex encode/decode at 8/16/32kHz.
//!
//! When compiled without the feature, stub implementations return errors.

use crate::translate::TranslateError;

// ---------------------------------------------------------------------------
// Native Speex FFI (feature = "native-speex")
// ---------------------------------------------------------------------------

#[cfg(feature = "native-speex")]
mod native {
    use std::os::raw::{c_int, c_void};

    /// Opaque Speex bits structure.
    #[repr(C)]
    pub struct SpeexBits {
        _private: [u8; 0],
    }

    extern "C" {
        // Mode getters
        pub fn speex_lib_get_mode(mode: c_int) -> *const c_void;

        // Encoder
        pub fn speex_encoder_init(mode: *const c_void) -> *mut c_void;
        pub fn speex_encoder_destroy(state: *mut c_void);
        pub fn speex_encode_int(
            state: *mut c_void,
            input: *const i16,
            bits: *mut SpeexBits,
        ) -> c_int;

        // Decoder
        pub fn speex_decoder_init(mode: *const c_void) -> *mut c_void;
        pub fn speex_decoder_destroy(state: *mut c_void);
        pub fn speex_decode_int(
            state: *mut c_void,
            bits: *mut SpeexBits,
            output: *mut i16,
        ) -> c_int;

        // Bits management
        pub fn speex_bits_init(bits: *mut SpeexBits);
        pub fn speex_bits_destroy(bits: *mut SpeexBits);
        pub fn speex_bits_reset(bits: *mut SpeexBits);
        pub fn speex_bits_write(
            bits: *mut SpeexBits,
            output: *mut u8,
            max_len: c_int,
        ) -> c_int;
        pub fn speex_bits_read_from(
            bits: *mut SpeexBits,
            data: *const u8,
            len: c_int,
        );

        // Control
        pub fn speex_encoder_ctl(
            state: *mut c_void,
            request: c_int,
            ptr: *mut c_void,
        ) -> c_int;
        pub fn speex_decoder_ctl(
            state: *mut c_void,
            request: c_int,
            ptr: *mut c_void,
        ) -> c_int;
    }

    // Speex mode constants
    pub const SPEEX_MODEID_NB: c_int = 0;
    pub const SPEEX_MODEID_WB: c_int = 1;
    pub const SPEEX_MODEID_UWB: c_int = 2;

    /// Safe wrapper around native Speex encoder.
    pub struct SafeSpeexEncoder {
        state: *mut c_void,
    }

    unsafe impl Send for SafeSpeexEncoder {}

    impl SafeSpeexEncoder {
        pub fn new(mode_id: c_int) -> Result<Self, super::TranslateError> {
            let mode = unsafe { speex_lib_get_mode(mode_id) };
            if mode.is_null() {
                return Err(super::TranslateError::Failed(
                    "speex_lib_get_mode returned null".into(),
                ));
            }
            let state = unsafe { speex_encoder_init(mode) };
            if state.is_null() {
                return Err(super::TranslateError::Failed(
                    "speex_encoder_init failed".into(),
                ));
            }
            Ok(Self { state })
        }
    }

    impl Drop for SafeSpeexEncoder {
        fn drop(&mut self) {
            unsafe {
                speex_encoder_destroy(self.state);
            }
        }
    }

    /// Safe wrapper around native Speex decoder.
    pub struct SafeSpeexDecoder {
        state: *mut c_void,
    }

    unsafe impl Send for SafeSpeexDecoder {}

    impl SafeSpeexDecoder {
        pub fn new(mode_id: c_int) -> Result<Self, super::TranslateError> {
            let mode = unsafe { speex_lib_get_mode(mode_id) };
            if mode.is_null() {
                return Err(super::TranslateError::Failed(
                    "speex_lib_get_mode returned null".into(),
                ));
            }
            let state = unsafe { speex_decoder_init(mode) };
            if state.is_null() {
                return Err(super::TranslateError::Failed(
                    "speex_decoder_init failed".into(),
                ));
            }
            Ok(Self { state })
        }
    }

    impl Drop for SafeSpeexDecoder {
        fn drop(&mut self) {
            unsafe {
                speex_decoder_destroy(self.state);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Stub implementation (no native-speex feature)
// ---------------------------------------------------------------------------

#[cfg(not(feature = "native-speex"))]
mod stub {
    use super::TranslateError;

    /// Stub Speex encoder.
    pub struct SafeSpeexEncoder;

    impl SafeSpeexEncoder {
        pub fn new(_mode_id: i32) -> Result<Self, TranslateError> {
            Err(TranslateError::Failed(
                "Native Speex not available. Build with --features native-speex".into(),
            ))
        }
    }

    /// Stub Speex decoder.
    pub struct SafeSpeexDecoder;

    impl SafeSpeexDecoder {
        pub fn new(_mode_id: i32) -> Result<Self, TranslateError> {
            Err(TranslateError::Failed(
                "Native Speex not available. Build with --features native-speex".into(),
            ))
        }
    }
}

// Re-export the active implementation
#[cfg(feature = "native-speex")]
pub use native::{SafeSpeexDecoder, SafeSpeexEncoder};

#[cfg(not(feature = "native-speex"))]
pub use stub::{SafeSpeexDecoder, SafeSpeexEncoder};

/// Check at runtime whether native Speex is available.
pub fn is_native_speex_available() -> bool {
    cfg!(feature = "native-speex")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_speex_availability() {
        #[cfg(not(feature = "native-speex"))]
        {
            assert!(!is_native_speex_available());
            assert!(SafeSpeexEncoder::new(0).is_err());
            assert!(SafeSpeexDecoder::new(0).is_err());
        }

        #[cfg(feature = "native-speex")]
        {
            assert!(is_native_speex_available());
        }
    }
}
