//! Stasis channel snooping.
//!
//! Port of `res/res_stasis_snoop.c`. Creates spy/whisper channels that can
//! listen to and/or inject audio into an existing channel. Used by ARI's
//! `channels/{id}/snoop` endpoint for call monitoring and barging.

use std::fmt;

use tracing::debug;

// ---------------------------------------------------------------------------
// Snoop direction
// ---------------------------------------------------------------------------

/// Direction for spy (listen) operations.
///
/// Mirrors `enum ast_audiohook_direction` from the C source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnoopDirection {
    /// No spying/whispering.
    None,
    /// Listen/whisper on the inbound (from caller) audio.
    In,
    /// Listen/whisper on the outbound (to caller) audio.
    Out,
    /// Listen/whisper on both directions (mixed).
    Both,
}

impl SnoopDirection {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::In => "in",
            Self::Out => "out",
            Self::Both => "both",
        }
    }

    pub fn from_str_value(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "in" => Self::In,
            "out" => Self::Out,
            "both" => Self::Both,
            _ => Self::None,
        }
    }

    /// Whether this direction is active (not None).
    pub fn is_active(&self) -> bool {
        !matches!(self, Self::None)
    }
}

impl fmt::Display for SnoopDirection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Snoop options
// ---------------------------------------------------------------------------

/// Options for creating a snoop channel.
#[derive(Debug, Clone)]
pub struct SnoopOptions {
    /// Direction for spying (listening).
    pub spy: SnoopDirection,
    /// Direction for whispering (audio injection).
    pub whisper: SnoopDirection,
    /// Stasis application to place the snoop channel into.
    pub app: String,
    /// Arguments to pass to the Stasis application.
    pub app_args: String,
    /// Snoop channel ID override (optional).
    pub snoop_id: Option<String>,
}

impl SnoopOptions {
    pub fn new(app: &str) -> Self {
        Self {
            spy: SnoopDirection::None,
            whisper: SnoopDirection::None,
            app: app.to_string(),
            app_args: String::new(),
            snoop_id: None,
        }
    }

    pub fn with_spy(mut self, direction: SnoopDirection) -> Self {
        self.spy = direction;
        self
    }

    pub fn with_whisper(mut self, direction: SnoopDirection) -> Self {
        self.whisper = direction;
        self
    }

    pub fn with_app_args(mut self, args: &str) -> Self {
        self.app_args = args.to_string();
        self
    }

    /// Validate that at least spy or whisper is enabled.
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.spy.is_active() && !self.whisper.is_active() {
            return Err("at least one of spy or whisper must be active");
        }
        if self.app.is_empty() {
            return Err("application name is required");
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Snoop channel
// ---------------------------------------------------------------------------

/// Timer interval in milliseconds for the snoop channel.
pub const SNOOP_INTERVAL_MS: u32 = 20;

/// Represents a snoop channel attached to a target channel.
///
/// Mirrors `struct stasis_app_snoop` from the C source.
#[derive(Debug, Clone)]
pub struct SnoopChannel {
    /// Unique ID of the snoop channel.
    pub snoop_channel_id: String,
    /// Channel ID being snooped on.
    pub target_channel_id: String,
    /// Spy direction.
    pub spy_direction: SnoopDirection,
    /// Whether spy is currently active.
    pub spy_active: bool,
    /// Whisper direction.
    pub whisper_direction: SnoopDirection,
    /// Whether whisper is currently active.
    pub whisper_active: bool,
    /// Stasis application name.
    pub app: String,
    /// Application arguments.
    pub app_args: String,
}

impl SnoopChannel {
    /// Create a new snoop channel from options.
    pub fn new(
        snoop_channel_id: &str,
        target_channel_id: &str,
        options: &SnoopOptions,
    ) -> Self {
        Self {
            snoop_channel_id: snoop_channel_id.to_string(),
            target_channel_id: target_channel_id.to_string(),
            spy_direction: options.spy,
            spy_active: options.spy.is_active(),
            whisper_direction: options.whisper,
            whisper_active: options.whisper.is_active(),
            app: options.app.clone(),
            app_args: options.app_args.clone(),
        }
    }

    /// Stop spying.
    pub fn stop_spy(&mut self) {
        self.spy_active = false;
        debug!(snoop = %self.snoop_channel_id, "Spy stopped");
    }

    /// Stop whispering.
    pub fn stop_whisper(&mut self) {
        self.whisper_active = false;
        debug!(snoop = %self.snoop_channel_id, "Whisper stopped");
    }

    /// Stop all snoop operations.
    pub fn stop(&mut self) {
        self.stop_spy();
        self.stop_whisper();
    }

    /// Whether the snoop channel is still doing anything useful.
    pub fn is_active(&self) -> bool {
        self.spy_active || self.whisper_active
    }
}

impl fmt::Display for SnoopChannel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Snoop({} -> {}, spy={}, whisper={})",
            self.snoop_channel_id,
            self.target_channel_id,
            self.spy_direction,
            self.whisper_direction,
        )
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snoop_direction() {
        assert!(SnoopDirection::Both.is_active());
        assert!(!SnoopDirection::None.is_active());
        assert_eq!(SnoopDirection::from_str_value("in"), SnoopDirection::In);
        assert_eq!(SnoopDirection::from_str_value("xyz"), SnoopDirection::None);
    }

    #[test]
    fn test_snoop_options_validation() {
        let opts = SnoopOptions::new("myapp");
        assert!(opts.validate().is_err());

        let opts = SnoopOptions::new("myapp").with_spy(SnoopDirection::Both);
        assert!(opts.validate().is_ok());
    }

    #[test]
    fn test_snoop_channel_lifecycle() {
        let opts = SnoopOptions::new("myapp")
            .with_spy(SnoopDirection::Both)
            .with_whisper(SnoopDirection::In);

        let mut snoop = SnoopChannel::new("snoop-001", "chan-001", &opts);
        assert!(snoop.is_active());
        assert!(snoop.spy_active);
        assert!(snoop.whisper_active);

        snoop.stop_spy();
        assert!(snoop.is_active()); // whisper still active

        snoop.stop_whisper();
        assert!(!snoop.is_active());
    }

    #[test]
    fn test_snoop_display() {
        let opts = SnoopOptions::new("myapp").with_spy(SnoopDirection::Both);
        let snoop = SnoopChannel::new("snoop-001", "chan-001", &opts);
        let display = format!("{}", snoop);
        assert!(display.contains("snoop-001"));
        assert!(display.contains("chan-001"));
    }
}
