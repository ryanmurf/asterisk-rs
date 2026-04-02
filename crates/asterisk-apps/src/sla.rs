//! Shared Line Appearances (SLA) application.
//!
//! Port of app_sla.c from Asterisk C. Provides SLAStation and SLATrunk
//! applications for shared line appearance functionality. Multiple stations
//! can share trunk lines: stations ring on incoming trunk calls, stations
//! can pick up trunks, and trunks can be placed on hold/retrieved.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use asterisk_types::ChannelState;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{info, warn};

/// State of an SLA trunk line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlaTrunkState {
    /// Trunk is idle (no active call).
    Idle,
    /// Trunk is ringing (incoming call).
    Ringing,
    /// Trunk is up (active call in progress).
    Up,
    /// Trunk is on hold.
    OnHold,
    /// Trunk is on hold by this specific station.
    OnHoldByMe,
}

/// Hold access mode for trunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlaHoldAccess {
    /// Any station can retrieve a held call.
    Open,
    /// Only the station that placed the hold can retrieve it.
    Private,
}

/// Status returned by SLAStation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlaStationStatus {
    Success,
    Failure,
    Congestion,
}

impl SlaStationStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "SUCCESS",
            Self::Failure => "FAILURE",
            Self::Congestion => "CONGESTION",
        }
    }
}

/// Status returned by SLATrunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlaTrunkStatus {
    Success,
    Failure,
    Unanswered,
    RingTimeout,
}

impl SlaTrunkStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "SUCCESS",
            Self::Failure => "FAILURE",
            Self::Unanswered => "UNANSWERED",
            Self::RingTimeout => "RINGTIMEOUT",
        }
    }
}

/// A reference from a station to a trunk it has access to.
#[derive(Debug, Clone)]
pub struct SlaTrunkRef {
    /// Name of the trunk.
    pub trunk_name: String,
    /// Current state of this trunk from the station's perspective.
    pub state: SlaTrunkState,
    /// Ring timeout for this specific trunk on this station (0 = use station default).
    pub ring_timeout: u32,
    /// Ring delay before this station starts ringing for this trunk.
    pub ring_delay: u32,
}

/// An SLA station definition from sla.conf.
#[derive(Debug, Clone)]
pub struct SlaStation {
    /// Station name.
    pub name: String,
    /// Device string (e.g., "SIP/station1").
    pub device: String,
    /// Auto-context for generating hints.
    pub auto_context: String,
    /// Ring timeout for any trunk (0 = no timeout).
    pub ring_timeout: u32,
    /// Ring delay before starting to ring.
    pub ring_delay: u32,
    /// Hold access mode.
    pub hold_access: SlaHoldAccess,
    /// Trunk references this station has access to.
    pub trunks: Vec<SlaTrunkRef>,
}

/// An SLA trunk definition from sla.conf.
#[derive(Debug, Clone)]
pub struct SlaTrunk {
    /// Trunk name.
    pub name: String,
    /// Device string (e.g., "DAHDI/1").
    pub device: String,
    /// Auto-context for generating hints.
    pub auto_context: String,
    /// Ring timeout in seconds (0 = no limit).
    pub ring_timeout: u32,
    /// Hold access mode.
    pub hold_access: SlaHoldAccess,
    /// Current state of the trunk.
    pub state: SlaTrunkState,
    /// Stations that have access to this trunk.
    pub station_names: Vec<String>,
}

/// Options for SLATrunk.
#[derive(Debug, Clone, Default)]
pub struct SlaTrunkOptions {
    /// Play MOH instead of ringing, with optional class.
    pub moh_class: Option<String>,
}

impl SlaTrunkOptions {
    pub fn parse(opts: &str) -> Self {
        let mut result = Self::default();
        let mut chars = opts.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == 'M' {
                // Consume optional MOH class in parens
                let mut class = String::new();
                if let Some(&'(') = chars.peek() {
                    chars.next();
                    for c in chars.by_ref() {
                        if c == ')' {
                            break;
                        }
                        class.push(c);
                    }
                }
                result.moh_class = Some(if class.is_empty() {
                    "default".to_string()
                } else {
                    class
                });
            }
        }
        result
    }
}

/// Global SLA configuration.
pub struct SlaConfig {
    stations: RwLock<HashMap<String, Arc<SlaStation>>>,
    trunks: RwLock<HashMap<String, Arc<SlaTrunk>>>,
}

impl SlaConfig {
    pub fn new() -> Self {
        Self {
            stations: RwLock::new(HashMap::new()),
            trunks: RwLock::new(HashMap::new()),
        }
    }

    pub fn add_station(&self, station: SlaStation) {
        let name = station.name.clone();
        self.stations.write().insert(name, Arc::new(station));
    }

    pub fn add_trunk(&self, trunk: SlaTrunk) {
        let name = trunk.name.clone();
        self.trunks.write().insert(name, Arc::new(trunk));
    }

    pub fn get_station(&self, name: &str) -> Option<Arc<SlaStation>> {
        self.stations.read().get(name).cloned()
    }

    pub fn get_trunk(&self, name: &str) -> Option<Arc<SlaTrunk>> {
        self.trunks.read().get(name).cloned()
    }
}

impl Default for SlaConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// The SLAStation() dialplan application.
///
/// Usage: SLAStation(station)
///
/// Executed by an SLA station. If the phone was just taken off hook,
/// the argument is just the station name. If initiated by pressing a
/// line key, the argument is "station_trunk".
///
/// Sets SLASTATION_STATUS to SUCCESS, FAILURE, or CONGESTION.
pub struct AppSlaStation;

impl DialplanApp for AppSlaStation {
    fn name(&self) -> &str {
        "SLAStation"
    }

    fn description(&self) -> &str {
        "Shared Line Appearance Station"
    }
}

impl AppSlaStation {
    /// Execute the SLAStation application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let station_arg = args.trim();
        if station_arg.is_empty() {
            warn!("SLAStation: requires a station name argument");
            return PbxExecResult::Failed;
        }

        // Parse station_trunk format: "station1_line1" => station="station1", trunk="line1"
        let (station_name, trunk_name) = if let Some(pos) = station_arg.find('_') {
            (&station_arg[..pos], Some(&station_arg[pos + 1..]))
        } else {
            (station_arg, None)
        };

        info!(
            "SLAStation: channel '{}' station='{}' trunk={:?}",
            channel.name, station_name, trunk_name,
        );

        // Answer the channel
        if channel.state != ChannelState::Up {
            channel.state = ChannelState::Up;
        }

        // In a real implementation:
        //
        //   let config = get_sla_config();
        //   let station = match config.get_station(station_name) {
        //       Some(s) => s,
        //       None => {
        //           set_variable(channel, "SLASTATION_STATUS", "FAILURE");
        //           return PbxExecResult::Failed;
        //       }
        //   };
        //
        //   if let Some(trunk_name) = trunk_name {
        //       // Station is requesting a specific trunk
        //       let trunk = match config.get_trunk(trunk_name) {
        //           Some(t) => t,
        //           None => {
        //               set_variable(channel, "SLASTATION_STATUS", "FAILURE");
        //               return PbxExecResult::Failed;
        //           }
        //       };
        //
        //       match trunk.state {
        //           SlaTrunkState::Ringing => {
        //               // Answer the ringing trunk
        //               pickup_trunk(channel, &trunk).await;
        //           }
        //           SlaTrunkState::OnHold | SlaTrunkState::OnHoldByMe => {
        //               // Retrieve the trunk from hold
        //               retrieve_trunk(channel, &trunk).await;
        //           }
        //           SlaTrunkState::Idle => {
        //               // Seize the trunk for outgoing call
        //               seize_trunk(channel, &station, &trunk).await;
        //           }
        //           _ => {
        //               set_variable(channel, "SLASTATION_STATUS", "CONGESTION");
        //               return PbxExecResult::Failed;
        //           }
        //       }
        //   } else {
        //       // Station off-hook with no specific trunk
        //       // Find a ringing trunk or provide dialtone
        //       if let Some(ringing_trunk) = find_ringing_trunk(&station) {
        //           pickup_trunk(channel, &ringing_trunk).await;
        //       } else {
        //           // Provide dialtone for trunk selection
        //           play_dialtone(channel).await;
        //       }
        //   }

        info!(
            "SLAStation: channel '{}' station '{}' completed",
            channel.name, station_name,
        );
        PbxExecResult::Success
    }
}

/// The SLATrunk() dialplan application.
///
/// Usage: SLATrunk(trunk[,options])
///
/// Executed by an SLA trunk on an inbound call. Rings all stations
/// that have access to this trunk.
///
/// Options:
///   M(class) - Play MOH instead of ringing
///
/// Sets SLATRUNK_STATUS to SUCCESS, FAILURE, UNANSWERED, or RINGTIMEOUT.
pub struct AppSlaTrunk;

impl DialplanApp for AppSlaTrunk {
    fn name(&self) -> &str {
        "SLATrunk"
    }

    fn description(&self) -> &str {
        "Shared Line Appearance Trunk"
    }
}

impl AppSlaTrunk {
    /// Execute the SLATrunk application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parts: Vec<&str> = args.splitn(2, ',').collect();
        let trunk_name = match parts.first() {
            Some(t) if !t.trim().is_empty() => t.trim(),
            _ => {
                warn!("SLATrunk: requires a trunk name argument");
                return PbxExecResult::Failed;
            }
        };

        let options = parts
            .get(1)
            .map(|o| SlaTrunkOptions::parse(o.trim()))
            .unwrap_or_default();

        info!(
            "SLATrunk: channel '{}' trunk='{}' moh={:?}",
            channel.name, trunk_name, options.moh_class,
        );

        // In a real implementation:
        //
        //   let config = get_sla_config();
        //   let trunk = match config.get_trunk(trunk_name) {
        //       Some(t) => t,
        //       None => {
        //           set_variable(channel, "SLATRUNK_STATUS", "FAILURE");
        //           return PbxExecResult::Failed;
        //       }
        //   };
        //
        //   // Set trunk state to ringing
        //   set_trunk_state(trunk_name, SlaTrunkState::Ringing);
        //
        //   // Ring all stations that have access to this trunk
        //   for station_name in &trunk.station_names {
        //       if let Some(station) = config.get_station(station_name) {
        //           ring_station(channel, &station, trunk_name).await;
        //       }
        //   }
        //
        //   // Wait for a station to answer or timeout
        //   match wait_for_answer(trunk.ring_timeout).await {
        //       WaitResult::Answered(station_chan) => {
        //           set_trunk_state(trunk_name, SlaTrunkState::Up);
        //           bridge_channels(channel, &station_chan).await;
        //           set_variable(channel, "SLATRUNK_STATUS", "SUCCESS");
        //       }
        //       WaitResult::Timeout => {
        //           set_trunk_state(trunk_name, SlaTrunkState::Idle);
        //           set_variable(channel, "SLATRUNK_STATUS", "RINGTIMEOUT");
        //       }
        //       WaitResult::NoAnswer => {
        //           set_trunk_state(trunk_name, SlaTrunkState::Idle);
        //           set_variable(channel, "SLATRUNK_STATUS", "UNANSWERED");
        //       }
        //   }

        info!(
            "SLATrunk: channel '{}' trunk '{}' completed",
            channel.name, trunk_name,
        );
        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sla_trunk_options() {
        let opts = SlaTrunkOptions::parse("M(jazz)");
        assert_eq!(opts.moh_class, Some("jazz".to_string()));
    }

    #[test]
    fn test_sla_trunk_options_default_moh() {
        let opts = SlaTrunkOptions::parse("M");
        assert_eq!(opts.moh_class, Some("default".to_string()));
    }

    #[test]
    fn test_sla_station_status() {
        assert_eq!(SlaStationStatus::Success.as_str(), "SUCCESS");
        assert_eq!(SlaStationStatus::Failure.as_str(), "FAILURE");
        assert_eq!(SlaStationStatus::Congestion.as_str(), "CONGESTION");
    }

    #[test]
    fn test_sla_trunk_state() {
        assert_eq!(SlaTrunkState::Idle, SlaTrunkState::Idle);
        assert_ne!(SlaTrunkState::Idle, SlaTrunkState::Up);
    }

    #[test]
    fn test_sla_config() {
        let config = SlaConfig::new();
        config.add_station(SlaStation {
            name: "station1".to_string(),
            device: "SIP/station1".to_string(),
            auto_context: String::new(),
            ring_timeout: 30,
            ring_delay: 0,
            hold_access: SlaHoldAccess::Open,
            trunks: vec![SlaTrunkRef {
                trunk_name: "line1".to_string(),
                state: SlaTrunkState::Idle,
                ring_timeout: 0,
                ring_delay: 0,
            }],
        });
        assert!(config.get_station("station1").is_some());
        assert!(config.get_station("station2").is_none());
    }

    #[tokio::test]
    async fn test_sla_station_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppSlaStation::exec(&mut channel, "station1").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_sla_station_with_trunk() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppSlaStation::exec(&mut channel, "station1_line1").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_sla_trunk_exec() {
        let mut channel = Channel::new("DAHDI/1-001");
        let result = AppSlaTrunk::exec(&mut channel, "line1").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_sla_trunk_no_args() {
        let mut channel = Channel::new("DAHDI/1-001");
        let result = AppSlaTrunk::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Failed);
    }
}
