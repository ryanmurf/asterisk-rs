//! ADSI (Analog Display Services Interface) support.
//!
//! Port of `res/res_adsi.c`. Stub implementation for ADSI CPE (Customer
//! Premises Equipment) control. ADSI allows visual display control on
//! analog phone sets using FSK (Frequency-Shift Keying) modem tones
//! during a call. Used by app_voicemail for visual voicemail menus.

use tracing::debug;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of retries for ADSI operations.
pub const MAX_RETRIES: u32 = 3;

/// Maximum number of speed dial entries.
pub const MAX_SPEED_DIAL: usize = 6;

/// ADSI flag indicating data mode.
pub const FLAG_DATAMODE: u32 = 1 << 8;

// ---------------------------------------------------------------------------
// Message types
// ---------------------------------------------------------------------------

/// ADSI message types for transmission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdsiMessageType {
    /// Display message (text on screen).
    Display,
    /// Download message (firmware/script).
    Download,
    /// Key configuration.
    KeyConfig,
}

// ---------------------------------------------------------------------------
// Text alignment
// ---------------------------------------------------------------------------

/// Text alignment for ADSI display lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdsiAlign {
    Left,
    Center,
    Right,
}

// ---------------------------------------------------------------------------
// ADSI operations (stub)
// ---------------------------------------------------------------------------

/// Begin an ADSI download session.
///
/// Stub - would send ADSI setup messages via FSK modem tones.
pub fn begin_download(
    _channel_id: &str,
    service: &str,
    _fdn: &[u8],
    _sec: &[u8],
    _version: u32,
) -> Result<(), &'static str> {
    debug!(service = service, "ADSI begin_download (stub)");
    Ok(())
}

/// End an ADSI download session.
pub fn end_download(_channel_id: &str) -> Result<(), &'static str> {
    debug!("ADSI end_download (stub)");
    Ok(())
}

/// Display text lines on the ADSI phone screen.
pub fn print_lines(
    _channel_id: &str,
    lines: &[(&str, AdsiAlign)],
    _voice: bool,
) -> Result<(), &'static str> {
    for (line, align) in lines {
        debug!(line = line, align = ?align, "ADSI print (stub)");
    }
    Ok(())
}

/// Load an ADSI session/application on the phone.
pub fn load_session(
    _channel_id: &str,
    app: &str,
    version: u32,
) -> Result<(), &'static str> {
    debug!(app = app, version = version, "ADSI load_session (stub)");
    Ok(())
}

/// Unload the current ADSI session from the phone.
pub fn unload_session(_channel_id: &str) -> Result<(), &'static str> {
    debug!("ADSI unload_session (stub)");
    Ok(())
}

/// Restore the channel to voice mode after ADSI data mode.
pub fn channel_restore(_channel_id: &str) -> Result<(), &'static str> {
    debug!("ADSI channel_restore (stub)");
    Ok(())
}

/// Transmit a raw ADSI message.
pub fn transmit_message(
    _channel_id: &str,
    _msg: &[u8],
    _msg_type: AdsiMessageType,
) -> Result<(), &'static str> {
    debug!("ADSI transmit_message (stub)");
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_begin_end_download() {
        assert!(begin_download("chan-001", "voicemail", &[], &[], 1).is_ok());
        assert!(end_download("chan-001").is_ok());
    }

    #[test]
    fn test_print_lines() {
        let lines = vec![
            ("Hello", AdsiAlign::Center),
            ("World", AdsiAlign::Left),
        ];
        assert!(print_lines("chan-001", &lines, false).is_ok());
    }

    #[test]
    fn test_session_lifecycle() {
        assert!(load_session("chan-001", "vmail", 1).is_ok());
        assert!(unload_session("chan-001").is_ok());
        assert!(channel_restore("chan-001").is_ok());
    }
}
