//! Asterisk File Format Handlers
//!
//! Provides file format readers and writers for audio files,
//! ported from Asterisk's formats/ directory.
//!
//! Supported formats:
//! - WAV (PCM, 8kHz/16kHz, 16-bit mono)
//! - SLN (raw signed linear, multiple sample rates)
//! - PCM (raw mu-law / A-law)
//! - GSM (GSM 06.10 container)

pub mod traits;
pub mod wav;
pub mod sln;
pub mod pcm;
pub mod gsm;
pub mod registry;
pub mod g729;
pub mod g723;
pub mod g726;
pub mod h263;
pub mod h264;
pub mod ilbc_format;
pub mod speex_format;
pub mod vox;
pub mod siren7;
pub mod siren14;

pub use traits::{FileFormat, FileStream, FileWriter};
pub use registry::{FormatRegistry, detect_format};
