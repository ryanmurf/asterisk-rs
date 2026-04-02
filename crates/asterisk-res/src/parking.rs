//! Call Parking resource module.
//!
//! Port of `res/res_parking.c` and `parking/res_parking.h`. Provides call
//! parking lots where calls can be placed on hold and retrieved by dialling
//! the assigned parking space extension.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use thiserror::Error;
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum ParkingError {
    #[error("parking lot not found: {0}")]
    LotNotFound(String),
    #[error("no available parking space in lot '{0}'")]
    NoSpace(String),
    #[error("parking space {0} not occupied in lot '{1}'")]
    SpaceNotOccupied(u32, String),
    #[error("parking space {0} already occupied in lot '{1}'")]
    SpaceOccupied(u32, String),
    #[error("parking lot configuration error: {0}")]
    Config(String),
    #[error("parking timeout for space {0} in lot '{1}'")]
    Timeout(u32, String),
}

pub type ParkingResult<T> = Result<T, ParkingError>;

// ---------------------------------------------------------------------------
// Slot assignment strategy
// ---------------------------------------------------------------------------

/// Strategy for assigning a parking space when no specific space is requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(Default)]
pub enum FindSlotStrategy {
    /// Always use the lowest available space.
    #[default]
    First,
    /// Track the last used space and use the next one.
    Next,
}

impl FindSlotStrategy {
    pub fn from_config(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "first" => Some(Self::First),
            "next" => Some(Self::Next),
            _ => None,
        }
    }
}


// ---------------------------------------------------------------------------
// Courtesy tone target
// ---------------------------------------------------------------------------

/// Who receives the courtesy tone when a parked call is retrieved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(Default)]
pub enum CourtesyToneTarget {
    No,
    #[default]
    Caller,
    Callee,
    Both,
}

impl CourtesyToneTarget {
    pub fn from_config(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "caller" => Self::Caller,
            "callee" => Self::Callee,
            "both" => Self::Both,
            _ => Self::No,
        }
    }
}


// ---------------------------------------------------------------------------
// Parking lot mode (lifecycle state)
// ---------------------------------------------------------------------------

/// Lifecycle state of a parking lot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParkingLotMode {
    /// Normal operational mode.
    Normal,
    /// Dynamically created.
    Dynamic,
    /// Disabled (pending removal).
    Disabled,
}

// ---------------------------------------------------------------------------
// Parked call
// ---------------------------------------------------------------------------

/// A call that is currently parked in a parking space.
///
/// Corresponds to `struct parked_user` in the C source.
#[derive(Debug, Clone)]
pub struct ParkedCall {
    /// Channel unique ID of the parked call.
    pub channel_id: String,
    /// Channel name of the parked call (e.g., "SIP/alice-0001").
    pub channel_name: String,
    /// Channel name of the entity that parked this call.
    pub parker_channel: String,
    /// Dial string to call back the parker on timeout.
    pub parker_dial_string: String,
    /// Assigned parking space number.
    pub space: u32,
    /// When this call was parked.
    pub parked_at: Instant,
    /// Timeout duration after which the call is returned to the parker.
    pub timeout: Duration,
    /// Name of the parking lot this call is in.
    pub lot_name: String,
}

impl ParkedCall {
    /// Check whether this parked call has timed out.
    pub fn is_timed_out(&self) -> bool {
        self.parked_at.elapsed() >= self.timeout
    }

    /// Remaining time before timeout.
    pub fn remaining(&self) -> Duration {
        self.timeout.saturating_sub(self.parked_at.elapsed())
    }
}

// ---------------------------------------------------------------------------
// Parking lot configuration
// ---------------------------------------------------------------------------

/// Configuration for a parking lot.
///
/// Corresponds to `struct parking_lot_cfg` in the C source.
#[derive(Debug, Clone)]
pub struct ParkingLotConfig {
    /// Name of the parking lot.
    pub name: String,
    /// Context for parking extensions.
    pub context: String,
    /// Extension used to park calls.
    pub park_ext: Option<String>,
    /// Whether the park extension is exclusive to this lot.
    pub park_ext_exclusive: bool,
    /// First parking space number.
    pub start_space: u32,
    /// Last parking space number (inclusive).
    pub end_space: u32,
    /// Parking timeout in seconds.
    pub timeout_secs: u32,
    /// Whether to call back the parker on timeout.
    pub comeback_to_origin: bool,
    /// Comeback dial time in seconds.
    pub comeback_dial_time: u32,
    /// Context for timed-out parked calls when `comeback_to_origin` is false.
    pub comeback_context: String,
    /// Music class to play to parked callers.
    pub music_class: String,
    /// Courtesy tone file.
    pub courtesy_tone: Option<String>,
    /// Who receives the courtesy tone.
    pub parked_play: CourtesyToneTarget,
    /// How to assign parking spaces.
    pub find_slot: FindSlotStrategy,
    /// Whether to add hints for parking spaces.
    pub parking_hints: bool,
}

impl Default for ParkingLotConfig {
    fn default() -> Self {
        Self {
            name: "default".to_string(),
            context: "parkedcalls".to_string(),
            park_ext: None,
            park_ext_exclusive: false,
            start_space: 701,
            end_space: 750,
            timeout_secs: 45,
            comeback_to_origin: true,
            comeback_dial_time: 30,
            comeback_context: "parkedcallstimeout".to_string(),
            music_class: String::new(),
            courtesy_tone: None,
            parked_play: CourtesyToneTarget::Caller,
            find_slot: FindSlotStrategy::First,
            parking_hints: false,
        }
    }
}

impl ParkingLotConfig {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            ..Default::default()
        }
    }

    /// Total number of parking spaces in this lot.
    pub fn capacity(&self) -> u32 {
        self.end_space.saturating_sub(self.start_space) + 1
    }

    /// Validate the configuration.
    pub fn validate(&self) -> ParkingResult<()> {
        if self.start_space > self.end_space {
            return Err(ParkingError::Config(format!(
                "start_space ({}) > end_space ({})",
                self.start_space, self.end_space
            )));
        }
        if self.timeout_secs == 0 {
            return Err(ParkingError::Config("timeout must be > 0".into()));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Parking lot
// ---------------------------------------------------------------------------

/// A parking lot containing zero or more parked calls.
///
/// Corresponds to `struct parking_lot` in the C source.
pub struct ParkingLot {
    /// Configuration for this lot.
    pub config: ParkingLotConfig,
    /// Lifecycle mode.
    pub mode: ParkingLotMode,
    /// Currently parked calls, keyed by parking space number.
    parked_calls: HashMap<u32, ParkedCall>,
    /// Last assigned parking space (for `FindSlotStrategy::Next`).
    last_space: u32,
}

impl ParkingLot {
    /// Create a new parking lot from configuration.
    pub fn new(config: ParkingLotConfig) -> Self {
        let last_space = config.start_space.saturating_sub(1);
        Self {
            config,
            mode: ParkingLotMode::Normal,
            parked_calls: HashMap::new(),
            last_space,
        }
    }

    /// Park a call in this lot, automatically assigning a space.
    ///
    /// Returns the assigned parking space number.
    pub fn park(
        &mut self,
        channel_id: &str,
        channel_name: &str,
        parker_channel: &str,
        parker_dial_string: &str,
    ) -> ParkingResult<u32> {
        let space = self.find_available_space()?;
        self.park_at_space(space, channel_id, channel_name, parker_channel, parker_dial_string)?;
        Ok(space)
    }

    /// Park a call at a specific space.
    pub fn park_at_space(
        &mut self,
        space: u32,
        channel_id: &str,
        channel_name: &str,
        parker_channel: &str,
        parker_dial_string: &str,
    ) -> ParkingResult<()> {
        if space < self.config.start_space || space > self.config.end_space {
            return Err(ParkingError::Config(format!(
                "Space {} outside lot range {}-{}",
                space, self.config.start_space, self.config.end_space
            )));
        }

        if self.parked_calls.contains_key(&space) {
            return Err(ParkingError::SpaceOccupied(space, self.config.name.clone()));
        }

        let call = ParkedCall {
            channel_id: channel_id.to_string(),
            channel_name: channel_name.to_string(),
            parker_channel: parker_channel.to_string(),
            parker_dial_string: parker_dial_string.to_string(),
            space,
            parked_at: Instant::now(),
            timeout: Duration::from_secs(self.config.timeout_secs as u64),
            lot_name: self.config.name.clone(),
        };

        debug!(
            lot = %self.config.name,
            space,
            channel = %channel_name,
            parker = %parker_channel,
            "Call parked"
        );

        self.parked_calls.insert(space, call);
        self.last_space = space;
        Ok(())
    }

    /// Retrieve a parked call from a specific space.
    ///
    /// Returns the parked call data and removes it from the lot.
    pub fn retrieve(&mut self, space: u32) -> ParkingResult<ParkedCall> {
        self.parked_calls.remove(&space).ok_or_else(|| {
            ParkingError::SpaceNotOccupied(space, self.config.name.clone())
        })
    }

    /// Find the first available parking space using the configured strategy.
    fn find_available_space(&self) -> ParkingResult<u32> {
        match self.config.find_slot {
            FindSlotStrategy::First => {
                for space in self.config.start_space..=self.config.end_space {
                    if !self.parked_calls.contains_key(&space) {
                        return Ok(space);
                    }
                }
            }
            FindSlotStrategy::Next => {
                let start = if self.last_space >= self.config.end_space {
                    self.config.start_space
                } else {
                    self.last_space + 1
                };

                // Scan from last_space+1 to end, then wrap around.
                for space in start..=self.config.end_space {
                    if !self.parked_calls.contains_key(&space) {
                        return Ok(space);
                    }
                }
                for space in self.config.start_space..start {
                    if !self.parked_calls.contains_key(&space) {
                        return Ok(space);
                    }
                }
            }
        }

        Err(ParkingError::NoSpace(self.config.name.clone()))
    }

    /// Get a reference to a parked call by space number.
    pub fn get_parked_call(&self, space: u32) -> Option<&ParkedCall> {
        self.parked_calls.get(&space)
    }

    /// Collect all timed-out parked calls, removing them from the lot.
    ///
    /// Returns the list of timed-out calls for comeback processing.
    pub fn collect_timeouts(&mut self) -> Vec<ParkedCall> {
        let timed_out_spaces: Vec<u32> = self
            .parked_calls
            .iter()
            .filter(|(_, call)| call.is_timed_out())
            .map(|(&space, _)| space)
            .collect();

        let mut timed_out = Vec::with_capacity(timed_out_spaces.len());
        for space in timed_out_spaces {
            if let Some(call) = self.parked_calls.remove(&space) {
                warn!(
                    lot = %self.config.name,
                    space,
                    channel = %call.channel_name,
                    "Parked call timed out"
                );
                timed_out.push(call);
            }
        }
        timed_out
    }

    /// Number of currently parked calls.
    pub fn parked_count(&self) -> usize {
        self.parked_calls.len()
    }

    /// Whether the lot has any available spaces.
    pub fn has_space(&self) -> bool {
        self.parked_count() < self.config.capacity() as usize
    }

    /// List all occupied parking spaces.
    pub fn occupied_spaces(&self) -> Vec<u32> {
        let mut spaces: Vec<u32> = self.parked_calls.keys().copied().collect();
        spaces.sort();
        spaces
    }

    /// Get all parked calls as an iterator.
    pub fn parked_calls(&self) -> impl Iterator<Item = &ParkedCall> {
        self.parked_calls.values()
    }
}

impl std::fmt::Debug for ParkingLot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParkingLot")
            .field("name", &self.config.name)
            .field("range", &format!("{}-{}", self.config.start_space, self.config.end_space))
            .field("parked_count", &self.parked_count())
            .field("mode", &self.mode)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Parking lot manager
// ---------------------------------------------------------------------------

/// Global manager for all parking lots.
pub struct ParkingManager {
    /// Parking lots keyed by name.
    lots: RwLock<HashMap<String, Arc<RwLock<ParkingLot>>>>,
    /// Global option: allow dynamic lot creation.
    pub parked_dynamic: bool,
}

impl ParkingManager {
    /// Create a new parking manager.
    pub fn new() -> Self {
        Self {
            lots: RwLock::new(HashMap::new()),
            parked_dynamic: false,
        }
    }

    /// Register a parking lot.
    pub fn register_lot(&self, lot: ParkingLot) {
        info!(
            name = %lot.config.name,
            range = %format!("{}-{}", lot.config.start_space, lot.config.end_space),
            "Registered parking lot"
        );
        self.lots.write().insert(
            lot.config.name.clone(),
            Arc::new(RwLock::new(lot)),
        );
    }

    /// Get a parking lot by name.
    pub fn get_lot(&self, name: &str) -> Option<Arc<RwLock<ParkingLot>>> {
        self.lots.read().get(name).cloned()
    }

    /// Unregister a parking lot by name.
    pub fn unregister_lot(&self, name: &str) -> bool {
        self.lots.write().remove(name).is_some()
    }

    /// List all registered lot names.
    pub fn lot_names(&self) -> Vec<String> {
        self.lots.read().keys().cloned().collect()
    }

    /// Park a call in the named lot (or default).
    pub fn park_call(
        &self,
        lot_name: &str,
        channel_id: &str,
        channel_name: &str,
        parker_channel: &str,
        parker_dial_string: &str,
    ) -> ParkingResult<(String, u32)> {
        let lot_name = if lot_name.is_empty() { "default" } else { lot_name };
        let lot = self.get_lot(lot_name)
            .ok_or_else(|| ParkingError::LotNotFound(lot_name.to_string()))?;

        let space = lot.write().park(
            channel_id,
            channel_name,
            parker_channel,
            parker_dial_string,
        )?;

        Ok((lot_name.to_string(), space))
    }

    /// Retrieve a parked call from a specific lot and space.
    pub fn retrieve_call(&self, lot_name: &str, space: u32) -> ParkingResult<ParkedCall> {
        let lot = self.get_lot(lot_name)
            .ok_or_else(|| ParkingError::LotNotFound(lot_name.to_string()))?;
        let result = lot.write().retrieve(space);
        result
    }

    /// Collect all timed-out calls across all lots.
    pub fn collect_all_timeouts(&self) -> Vec<ParkedCall> {
        let lots = self.lots.read();
        let mut all_timeouts = Vec::new();
        for lot in lots.values() {
            all_timeouts.extend(lot.write().collect_timeouts());
        }
        all_timeouts
    }
}

impl Default for ParkingManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_lot(name: &str, start: u32, end: u32) -> ParkingLot {
        let config = ParkingLotConfig {
            name: name.to_string(),
            start_space: start,
            end_space: end,
            timeout_secs: 45,
            ..Default::default()
        };
        ParkingLot::new(config)
    }

    #[test]
    fn test_parking_lot_config_defaults() {
        let cfg = ParkingLotConfig::default();
        assert_eq!(cfg.start_space, 701);
        assert_eq!(cfg.end_space, 750);
        assert_eq!(cfg.timeout_secs, 45);
        assert!(cfg.comeback_to_origin);
        assert_eq!(cfg.capacity(), 50);
    }

    #[test]
    fn test_parking_lot_config_validate() {
        let mut cfg = ParkingLotConfig::default();
        assert!(cfg.validate().is_ok());

        cfg.start_space = 800;
        cfg.end_space = 700;
        assert!(cfg.validate().is_err());

        cfg.start_space = 700;
        cfg.end_space = 800;
        cfg.timeout_secs = 0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_park_and_retrieve() {
        let mut lot = make_lot("test", 701, 710);
        let space = lot.park("chan-1", "SIP/alice-001", "SIP/bob-001", "SIP/bob").unwrap();
        assert_eq!(space, 701);
        assert_eq!(lot.parked_count(), 1);

        let call = lot.retrieve(701).unwrap();
        assert_eq!(call.channel_id, "chan-1");
        assert_eq!(call.channel_name, "SIP/alice-001");
        assert_eq!(call.parker_channel, "SIP/bob-001");
        assert_eq!(lot.parked_count(), 0);
    }

    #[test]
    fn test_park_multiple() {
        let mut lot = make_lot("test", 701, 710);
        let s1 = lot.park("c1", "SIP/a-001", "SIP/b-001", "SIP/b").unwrap();
        let s2 = lot.park("c2", "SIP/a-002", "SIP/b-002", "SIP/b").unwrap();
        let s3 = lot.park("c3", "SIP/a-003", "SIP/b-003", "SIP/b").unwrap();

        assert_eq!(s1, 701);
        assert_eq!(s2, 702);
        assert_eq!(s3, 703);
        assert_eq!(lot.parked_count(), 3);
    }

    #[test]
    fn test_park_full_lot() {
        let mut lot = make_lot("tiny", 701, 702);
        lot.park("c1", "SIP/a-001", "SIP/b-001", "SIP/b").unwrap();
        lot.park("c2", "SIP/a-002", "SIP/b-002", "SIP/b").unwrap();

        let result = lot.park("c3", "SIP/a-003", "SIP/b-003", "SIP/b");
        assert!(matches!(result, Err(ParkingError::NoSpace(_))));
    }

    #[test]
    fn test_retrieve_empty_space() {
        let mut lot = make_lot("test", 701, 710);
        let result = lot.retrieve(701);
        assert!(matches!(result, Err(ParkingError::SpaceNotOccupied(701, _))));
    }

    #[test]
    fn test_find_slot_next_strategy() {
        let config = ParkingLotConfig {
            name: "next".to_string(),
            start_space: 701,
            end_space: 705,
            find_slot: FindSlotStrategy::Next,
            ..Default::default()
        };
        let mut lot = ParkingLot::new(config);

        let s1 = lot.park("c1", "a", "b", "b").unwrap();
        let s2 = lot.park("c2", "a", "b", "b").unwrap();
        assert_eq!(s1, 701);
        assert_eq!(s2, 702);

        // Free space 701.
        lot.retrieve(701).unwrap();

        // Next should be 703 (not 701, because Next strategy continues forward).
        let s3 = lot.park("c3", "a", "b", "b").unwrap();
        assert_eq!(s3, 703);
    }

    #[test]
    fn test_occupied_spaces() {
        let mut lot = make_lot("test", 701, 710);
        lot.park("c1", "a", "b", "b").unwrap();
        lot.park("c2", "a", "b", "b").unwrap();
        lot.park("c3", "a", "b", "b").unwrap();
        lot.retrieve(702).unwrap();

        let occupied = lot.occupied_spaces();
        assert_eq!(occupied, vec![701, 703]);
    }

    #[test]
    fn test_parking_manager_basic() {
        let mgr = ParkingManager::new();
        mgr.register_lot(make_lot("default", 701, 750));

        let (lot_name, space) = mgr
            .park_call("default", "c1", "SIP/a-001", "SIP/b-001", "SIP/b")
            .unwrap();
        assert_eq!(lot_name, "default");
        assert_eq!(space, 701);

        let call = mgr.retrieve_call("default", 701).unwrap();
        assert_eq!(call.channel_id, "c1");
    }

    #[test]
    fn test_parking_manager_lot_not_found() {
        let mgr = ParkingManager::new();
        let result = mgr.park_call("nonexistent", "c1", "a", "b", "b");
        assert!(matches!(result, Err(ParkingError::LotNotFound(_))));
    }

    #[test]
    fn test_parked_call_timeout() {
        let call = ParkedCall {
            channel_id: "c1".to_string(),
            channel_name: "SIP/a".to_string(),
            parker_channel: "SIP/b".to_string(),
            parker_dial_string: "SIP/b".to_string(),
            space: 701,
            parked_at: Instant::now() - Duration::from_secs(60),
            timeout: Duration::from_secs(45),
            lot_name: "default".to_string(),
        };
        assert!(call.is_timed_out());
        assert_eq!(call.remaining(), Duration::ZERO);
    }

    #[test]
    fn test_courtesy_tone_target_parse() {
        assert_eq!(CourtesyToneTarget::from_config("caller"), CourtesyToneTarget::Caller);
        assert_eq!(CourtesyToneTarget::from_config("callee"), CourtesyToneTarget::Callee);
        assert_eq!(CourtesyToneTarget::from_config("both"), CourtesyToneTarget::Both);
        assert_eq!(CourtesyToneTarget::from_config("no"), CourtesyToneTarget::No);
    }
}
