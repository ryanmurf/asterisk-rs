//! GSM 06.10 FFI bridge with feature-gated native codec support.
//!
//! When compiled with `--features native-gsm`, this module provides
//! FFI bindings to libgsm for GSM full-rate encode/decode.
//!
//! When compiled without the feature, stub implementations return errors.

use crate::translate::TranslateError;

/// GSM frame size in bytes (33 bytes per 20ms frame).
pub const GSM_FRAME_SIZE: usize = 33;
/// GSM frame duration in samples at 8kHz (160 samples = 20ms).
pub const GSM_SAMPLES_PER_FRAME: usize = 160;

// ---------------------------------------------------------------------------
// Native GSM FFI (feature = "native-gsm")
// ---------------------------------------------------------------------------

#[cfg(feature = "native-gsm")]
mod native {
    use std::os::raw::c_int;

    /// Opaque GSM state handle.
    #[repr(C)]
    pub struct GsmState {
        _private: [u8; 0],
    }

    extern "C" {
        pub fn gsm_create() -> *mut GsmState;
        pub fn gsm_destroy(state: *mut GsmState);
        pub fn gsm_encode(
            state: *mut GsmState,
            source: *const i16,
            dest: *mut u8,
        );
        pub fn gsm_decode(
            state: *mut GsmState,
            source: *const u8,
            dest: *mut i16,
        ) -> c_int;
    }

    /// Safe wrapper around native GSM encoder.
    pub struct SafeGsmEncoder {
        ptr: *mut GsmState,
    }

    unsafe impl Send for SafeGsmEncoder {}

    impl SafeGsmEncoder {
        pub fn new() -> Result<Self, super::TranslateError> {
            let ptr = unsafe { gsm_create() };
            if ptr.is_null() {
                return Err(super::TranslateError::Failed(
                    "gsm_create failed".into(),
                ));
            }
            Ok(Self { ptr })
        }

        /// Encode 160 PCM samples (20ms at 8kHz) to 33 bytes of GSM data.
        pub fn encode(&mut self, pcm: &[i16; 160], output: &mut [u8; 33]) {
            unsafe {
                gsm_encode(self.ptr, pcm.as_ptr(), output.as_mut_ptr());
            }
        }
    }

    impl Drop for SafeGsmEncoder {
        fn drop(&mut self) {
            unsafe {
                gsm_destroy(self.ptr);
            }
        }
    }

    /// Safe wrapper around native GSM decoder.
    pub struct SafeGsmDecoder {
        ptr: *mut GsmState,
    }

    unsafe impl Send for SafeGsmDecoder {}

    impl SafeGsmDecoder {
        pub fn new() -> Result<Self, super::TranslateError> {
            let ptr = unsafe { gsm_create() };
            if ptr.is_null() {
                return Err(super::TranslateError::Failed(
                    "gsm_create failed".into(),
                ));
            }
            Ok(Self { ptr })
        }

        /// Decode 33 bytes of GSM data to 160 PCM samples.
        pub fn decode(
            &mut self,
            data: &[u8; 33],
            output: &mut [i16; 160],
        ) -> Result<(), super::TranslateError> {
            let result = unsafe {
                gsm_decode(self.ptr, data.as_ptr(), output.as_mut_ptr())
            };
            if result != 0 {
                return Err(super::TranslateError::Failed(format!(
                    "gsm_decode failed with error {}",
                    result
                )));
            }
            Ok(())
        }
    }

    impl Drop for SafeGsmDecoder {
        fn drop(&mut self) {
            unsafe {
                gsm_destroy(self.ptr);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Stub implementation (no native-gsm feature)
// ---------------------------------------------------------------------------

#[cfg(not(feature = "native-gsm"))]
mod stub {
    use super::TranslateError;

    /// Stub GSM encoder.
    pub struct SafeGsmEncoder;

    impl SafeGsmEncoder {
        pub fn new() -> Result<Self, TranslateError> {
            Err(TranslateError::Failed(
                "Native GSM not available. Build with --features native-gsm".into(),
            ))
        }

        pub fn encode(&mut self, _pcm: &[i16; 160], _output: &mut [u8; 33]) {
            // unreachable in stub
        }
    }

    /// Stub GSM decoder.
    pub struct SafeGsmDecoder;

    impl SafeGsmDecoder {
        pub fn new() -> Result<Self, TranslateError> {
            Err(TranslateError::Failed(
                "Native GSM not available. Build with --features native-gsm".into(),
            ))
        }

        pub fn decode(
            &mut self,
            _data: &[u8; 33],
            _output: &mut [i16; 160],
        ) -> Result<(), TranslateError> {
            Err(TranslateError::Failed(
                "Native GSM not available".into(),
            ))
        }
    }
}

// Re-export the active implementation
#[cfg(feature = "native-gsm")]
pub use native::{SafeGsmDecoder, SafeGsmEncoder};

#[cfg(not(feature = "native-gsm"))]
pub use stub::{SafeGsmDecoder, SafeGsmEncoder};

/// Check at runtime whether native GSM is available.
pub fn is_native_gsm_available() -> bool {
    cfg!(feature = "native-gsm")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gsm_availability() {
        #[cfg(not(feature = "native-gsm"))]
        {
            assert!(!is_native_gsm_available());
            assert!(SafeGsmEncoder::new().is_err());
            assert!(SafeGsmDecoder::new().is_err());
        }

        #[cfg(feature = "native-gsm")]
        {
            assert!(is_native_gsm_available());
        }
    }

    #[test]
    fn test_gsm_constants() {
        assert_eq!(GSM_FRAME_SIZE, 33);
        assert_eq!(GSM_SAMPLES_PER_FRAME, 160);
    }
}
