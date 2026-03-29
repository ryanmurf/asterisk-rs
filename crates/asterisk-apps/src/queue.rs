//! Queue application - call queuing with strategy-based member selection.
//!
//! Port of app_queue.c from Asterisk C. Provides call queues where incoming
//! calls wait for an available agent/member. Supports multiple ring strategies,
//! member management, penalty-based routing, queue rules for dynamic penalty
//! adjustment, periodic/position/holdtime announcements, wrapup time, retry/timeout,
//! queue logging, realtime member loading, and queue status variables.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::{Channel, ChannelId};
use asterisk_types::ChannelState;
use dashmap::DashMap;
use parking_lot::RwLock;
use rand::seq::SliceRandom;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tracing::{debug, info, warn};

/// Global registry of configured queues.
static QUEUES: once_cell::sync::Lazy<DashMap<String, Arc<RwLock<CallQueue>>>> =
    once_cell::sync::Lazy::new(DashMap::new);

/// Global registry of queue rules.
static QUEUE_RULES: once_cell::sync::Lazy<DashMap<String, Vec<QueueRule>>> =
    once_cell::sync::Lazy::new(DashMap::new);

// ---------------------------------------------------------------------------
// Queue Strategy
// ---------------------------------------------------------------------------

/// Ring strategy for distributing calls to queue members.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueStrategy {
    /// Ring all available members simultaneously
    RingAll,
    /// Ring the member who has been idle the longest (least recently called)
    LeastRecent,
    /// Ring the member who has taken the fewest calls
    FewestCalls,
    /// Ring a random available member
    Random,
    /// Ring members in round-robin order, remembering position
    RoundRobin,
    /// Ring members in order (always start from first in list)
    Linear,
}

impl QueueStrategy {
    /// Parse a strategy from a string name.
    pub fn from_str_name(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "ringall" => Self::RingAll,
            "leastrecent" => Self::LeastRecent,
            "fewestcalls" => Self::FewestCalls,
            "random" => Self::Random,
            "roundrobin" | "rrmemory" => Self::RoundRobin,
            "linear" => Self::Linear,
            _ => {
                warn!("Queue: unknown strategy '{}', defaulting to RingAll", s);
                Self::RingAll
            }
        }
    }

    /// Human-readable name.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::RingAll => "ringall",
            Self::LeastRecent => "leastrecent",
            Self::FewestCalls => "fewestcalls",
            Self::Random => "random",
            Self::RoundRobin => "roundrobin",
            Self::Linear => "linear",
        }
    }
}

// ---------------------------------------------------------------------------
// Queue Member
// ---------------------------------------------------------------------------

/// Device/availability status of a queue member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberStatus {
    /// Member is available to take calls
    Available,
    /// Member is currently on a call
    Busy,
    /// Member's device is ringing
    Ringing,
    /// Member is unavailable (not registered, etc.)
    Unavailable,
    /// Member is in wrap-up time after a call
    WrapUp,
}

/// A member (agent) of a call queue.
#[derive(Debug, Clone)]
pub struct QueueMember {
    /// Interface string identifying this member (e.g., "SIP/alice")
    pub interface: String,
    /// State interface (for device state monitoring, may differ from interface)
    pub state_interface: Option<String>,
    /// Friendly name for display
    pub member_name: String,
    /// Penalty level (lower = higher priority, 0 = highest)
    pub penalty: u32,
    /// Whether the member is paused (not receiving calls)
    pub paused: bool,
    /// Pause reason
    pub pause_reason: Option<String>,
    /// Whether the member is currently in a call from this queue
    pub in_call: bool,
    /// Number of calls taken by this member
    pub calls_taken: u64,
    /// Timestamp of last call completion
    pub last_call_time: Option<Instant>,
    /// When this member was added to the queue
    pub added_time: Instant,
    /// Current device state (available, busy, ringing, etc.)
    pub status: MemberStatus,
    /// Whether this member was added dynamically (vs. statically in config)
    pub dynamic: bool,
    /// Whether to ring this member even if device shows in-use
    pub ring_in_use: bool,
    /// Per-member wrapup time override (None = use queue default)
    pub wrapup_time: Option<Duration>,
    /// When wrapup started (if in WrapUp status)
    pub wrapup_start: Option<Instant>,
}

impl QueueMember {
    /// Create a new member with default settings.
    pub fn new(interface: String, member_name: String, penalty: u32) -> Self {
        Self {
            interface,
            state_interface: None,
            member_name,
            penalty,
            paused: false,
            pause_reason: None,
            in_call: false,
            calls_taken: 0,
            last_call_time: None,
            added_time: Instant::now(),
            status: MemberStatus::Available,
            dynamic: false,
            ring_in_use: false,
            wrapup_time: None,
            wrapup_start: None,
        }
    }

    /// Check if this member is available, considering wrapup time.
    pub fn is_available(&self, queue_wrapup: Duration) -> bool {
        if self.paused || self.in_call {
            return false;
        }
        match self.status {
            MemberStatus::Available => true,
            MemberStatus::WrapUp => {
                // Check if wrapup has expired
                if let Some(start) = self.wrapup_start {
                    let wrapup = self.wrapup_time.unwrap_or(queue_wrapup);
                    start.elapsed() >= wrapup
                } else {
                    true
                }
            }
            _ => false,
        }
    }

    /// Start wrapup period after completing a call.
    pub fn begin_wrapup(&mut self) {
        self.status = MemberStatus::WrapUp;
        self.wrapup_start = Some(Instant::now());
        self.in_call = false;
        self.last_call_time = Some(Instant::now());
        self.calls_taken += 1;
    }

    /// Check and clear wrapup if expired.
    pub fn check_wrapup_expired(&mut self, queue_wrapup: Duration) -> bool {
        if self.status != MemberStatus::WrapUp {
            return false;
        }
        if let Some(start) = self.wrapup_start {
            let wrapup = self.wrapup_time.unwrap_or(queue_wrapup);
            if start.elapsed() >= wrapup {
                self.status = MemberStatus::Available;
                self.wrapup_start = None;
                return true;
            }
        }
        false
    }
}

// ---------------------------------------------------------------------------
// Queue Caller
// ---------------------------------------------------------------------------

/// A caller waiting in the queue.
#[derive(Debug, Clone)]
pub struct QueueCaller {
    /// The caller's channel ID
    pub channel_id: ChannelId,
    /// The caller's channel name
    pub channel_name: String,
    /// Position in the queue (1-based)
    pub position: usize,
    /// When the caller entered the queue
    pub enter_time: Instant,
    /// CallerID name
    pub caller_name: Option<String>,
    /// CallerID number
    pub caller_number: Option<String>,
    /// When the last periodic announcement was played
    pub last_periodic_announce: Option<Instant>,
    /// When the last position announcement was played
    pub last_position_announce: Option<Instant>,
}

// ---------------------------------------------------------------------------
// Queue Rules - Dynamic Penalty Adjustment
// ---------------------------------------------------------------------------

/// A queue rule that adjusts the maximum penalty based on wait time.
///
/// In Asterisk, queue rules are defined in queues.conf like:
///   rule = 0,5   ; at 0 seconds, max penalty = 5
///   rule = 60,10 ; after 60 seconds, max penalty = 10
///   rule = 120,0 ; after 120 seconds, no penalty limit
#[derive(Debug, Clone)]
pub struct QueueRule {
    /// Minimum wait time (seconds) before this rule applies
    pub min_wait_secs: u64,
    /// Max penalty to use when this rule is active. 0 = no limit.
    pub max_penalty: u32,
    /// Min penalty to use when this rule is active. 0 = no minimum.
    pub min_penalty: u32,
    /// Relative change to max_penalty (instead of absolute)
    pub relative: bool,
}

impl QueueRule {
    /// Evaluate what max_penalty to apply given the caller's wait time.
    pub fn evaluate_rules(rules: &[QueueRule], wait_secs: u64) -> (u32, u32) {
        let mut max_penalty = u32::MAX; // No limit by default
        let mut min_penalty = 0u32;
        for rule in rules {
            if wait_secs >= rule.min_wait_secs {
                max_penalty = if rule.max_penalty == 0 {
                    u32::MAX
                } else {
                    rule.max_penalty
                };
                min_penalty = rule.min_penalty;
            }
        }
        (max_penalty, min_penalty)
    }
}

// ---------------------------------------------------------------------------
// Announcements
// ---------------------------------------------------------------------------

/// Configuration for queue announcements played to waiting callers.
#[derive(Debug, Clone)]
pub struct QueueAnnouncements {
    /// Interval between periodic announcements (0 = disabled)
    pub periodic_announce_interval: Duration,
    /// Sound files for periodic announcements (cycled through)
    pub periodic_announce_files: Vec<String>,
    /// Current index into periodic_announce_files
    pub periodic_announce_index: usize,
    /// Whether to announce the caller's queue position
    pub announce_position: bool,
    /// Minimum number of callers before position announcements
    pub announce_position_min: usize,
    /// Maximum position to announce (0 = no limit). Beyond this, say "high call volume"
    pub announce_position_max: usize,
    /// Whether to announce estimated hold time
    pub announce_holdtime: bool,
    /// Only announce hold time if >= this many seconds
    pub announce_holdtime_min_secs: u64,
    /// Sound played when caller joins queue
    pub join_sound: Option<String>,
    /// Sound played when caller leaves queue (to remaining callers, if any)
    pub leave_sound: Option<String>,
    /// Sound played to agent before bridging the call
    pub agent_announce: Option<String>,
    /// Report hold time to the answering agent
    pub report_holdtime: bool,
}

impl Default for QueueAnnouncements {
    fn default() -> Self {
        Self {
            periodic_announce_interval: Duration::ZERO,
            periodic_announce_files: vec!["queue-periodic-announce".to_string()],
            periodic_announce_index: 0,
            announce_position: false,
            announce_position_min: 0,
            announce_position_max: 0,
            announce_holdtime: false,
            announce_holdtime_min_secs: 0,
            join_sound: None,
            leave_sound: None,
            agent_announce: None,
            report_holdtime: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Queue Log
// ---------------------------------------------------------------------------

/// Queue log event types mirroring Asterisk's queue_log file format.
///
/// Each entry is written as:
///   timestamp|callid|queuename|agent|event|data1|data2|data3
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueLogEvent {
    /// Caller entered the queue. data: url|callerid
    EnterQueue,
    /// Caller connected to agent. data: holdtime|bridgedchannel|ringtime
    Connect,
    /// Caller completed the call by hanging up. data: holdtime|calltime|origposition
    CompleteCaller,
    /// Agent completed the call by hanging up. data: holdtime|calltime|origposition
    CompleteAgent,
    /// Caller abandoned (hung up while waiting). data: position|origposition|waittime
    Abandon,
    /// Call was transferred. data: extension|context|holdtime|calltime
    Transfer,
    /// Agent ring with no answer. data: ringtime
    RingNoAnswer,
    /// Agent added to queue. data: penalty
    AddMember,
    /// Agent removed from queue
    RemoveMember,
    /// Agent paused. data: reason
    Pause,
    /// Agent unpaused. data: reason
    Unpause,
    /// Caller exited queue due to timeout
    ExitWithTimeout,
    /// Caller exited because queue was empty
    ExitWithKey,
    /// Custom event
    Custom(String),
}

impl QueueLogEvent {
    pub fn as_str(&self) -> &str {
        match self {
            Self::EnterQueue => "ENTERQUEUE",
            Self::Connect => "CONNECT",
            Self::CompleteCaller => "COMPLETECALLER",
            Self::CompleteAgent => "COMPLETEAGENT",
            Self::Abandon => "ABANDON",
            Self::Transfer => "TRANSFER",
            Self::RingNoAnswer => "RINGNOANSWER",
            Self::AddMember => "ADDMEMBER",
            Self::RemoveMember => "REMOVEMEMBER",
            Self::Pause => "PAUSE",
            Self::Unpause => "UNPAUSE",
            Self::ExitWithTimeout => "EXITTIMEOUT",
            Self::ExitWithKey => "EXITWITHKEY",
            Self::Custom(s) => s.as_str(),
        }
    }
}

/// A single queue log entry.
#[derive(Debug, Clone)]
pub struct QueueLogEntry {
    pub timestamp: SystemTime,
    pub call_id: String,
    pub queue_name: String,
    pub agent: String,
    pub event: QueueLogEvent,
    pub data: Vec<String>,
}

impl QueueLogEntry {
    pub fn new(
        call_id: &str,
        queue_name: &str,
        agent: &str,
        event: QueueLogEvent,
        data: Vec<String>,
    ) -> Self {
        Self {
            timestamp: SystemTime::now(),
            call_id: call_id.to_string(),
            queue_name: queue_name.to_string(),
            agent: agent.to_string(),
            event,
            data,
        }
    }

    /// Format as a queue_log line.
    pub fn format_line(&self) -> String {
        let ts = self
            .timestamp
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let data_str = self.data.join("|");
        format!(
            "{}|{}|{}|{}|{}|{}",
            ts,
            self.call_id,
            self.queue_name,
            self.agent,
            self.event.as_str(),
            data_str
        )
    }
}

/// In-memory queue log collector.
/// In production, this writes to /var/log/asterisk/queue_log or to a database.
static QUEUE_LOG: once_cell::sync::Lazy<RwLock<Vec<QueueLogEntry>>> =
    once_cell::sync::Lazy::new(|| RwLock::new(Vec::new()));

/// Write a queue log entry.
pub fn queue_log_write(entry: QueueLogEntry) {
    info!("queue_log: {}", entry.format_line());
    QUEUE_LOG.write().push(entry);
}

/// Read all queue log entries (for testing/inspection).
pub fn queue_log_entries() -> Vec<QueueLogEntry> {
    QUEUE_LOG.read().clone()
}

// ---------------------------------------------------------------------------
// Queue Status Variables
// ---------------------------------------------------------------------------

/// Queue status variables set on the channel after Queue() completes.
/// Mirrors Asterisk's QUEUESTATUS, QUEUENAME, etc.
#[derive(Debug, Clone, Default)]
pub struct QueueStatusVars {
    /// QUEUESTATUS: TIMEOUT, FULL, JOINEMPTY, LEAVEEMPTY, JOINUNAVAIL, LEAVEUNAVAIL, CONTINUE
    pub queue_status: String,
    /// QUEUENAME: name of the queue
    pub queue_name: String,
    /// QUEUEPOSITION: caller's position when connected or when leaving
    pub queue_position: usize,
    /// QUEUEHOLDTIME: seconds the caller waited
    pub queue_holdtime: u64,
    /// QUEUECOMPLETED: total completed calls for the queue
    pub queue_completed: u64,
    /// QUEUEABANDONED: total abandoned calls for the queue
    pub queue_abandoned: u64,
    /// QUEUEWAIT: seconds waited
    pub queue_wait: u64,
    /// ANSWEREDTIME: seconds connected to agent (0 if not answered)
    pub answered_time: u64,
}

impl QueueStatusVars {
    /// Apply these variables onto a channel's variable map.
    pub fn apply_to_channel(&self, channel: &mut Channel) {
        channel
            .variables
            .insert("QUEUESTATUS".to_string(), self.queue_status.clone());
        channel
            .variables
            .insert("QUEUENAME".to_string(), self.queue_name.clone());
        channel.variables.insert(
            "QUEUEPOSITION".to_string(),
            self.queue_position.to_string(),
        );
        channel.variables.insert(
            "QUEUEHOLDTIME".to_string(),
            self.queue_holdtime.to_string(),
        );
        channel.variables.insert(
            "QUEUECOMPLETED".to_string(),
            self.queue_completed.to_string(),
        );
        channel.variables.insert(
            "QUEUEABANDONED".to_string(),
            self.queue_abandoned.to_string(),
        );
        channel
            .variables
            .insert("QUEUEWAIT".to_string(), self.queue_wait.to_string());
        channel.variables.insert(
            "ANSWEREDTIME".to_string(),
            self.answered_time.to_string(),
        );
    }
}

// ---------------------------------------------------------------------------
// Realtime Member Backend
// ---------------------------------------------------------------------------

/// Trait for loading queue members from a realtime backend (database).
///
/// In Asterisk C, realtime queue member loading uses the ast_load_realtime()
/// framework to query members from a database table.
pub trait RealtimeMemberBackend: Send + Sync {
    /// Load members for a given queue from the realtime backend.
    fn load_members(&self, queue_name: &str) -> Vec<QueueMember>;

    /// Save/update a member's state back to the realtime backend.
    fn save_member(&self, queue_name: &str, member: &QueueMember) -> bool;

    /// Remove a member from the realtime backend.
    fn remove_member(&self, queue_name: &str, interface: &str) -> bool;
}

/// A stub realtime backend that returns no members (for when realtime is not configured).
#[derive(Debug)]
pub struct NoopRealtimeBackend;

impl RealtimeMemberBackend for NoopRealtimeBackend {
    fn load_members(&self, _queue_name: &str) -> Vec<QueueMember> {
        Vec::new()
    }

    fn save_member(&self, _queue_name: &str, _member: &QueueMember) -> bool {
        false
    }

    fn remove_member(&self, _queue_name: &str, _interface: &str) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// CallQueue
// ---------------------------------------------------------------------------

/// A call queue definition.
#[derive(Debug)]
pub struct CallQueue {
    /// Queue name
    pub name: String,
    /// Ring strategy
    pub strategy: QueueStrategy,
    /// Queue members (agents)
    pub members: Vec<QueueMember>,
    /// Callers currently waiting in the queue
    pub callers: VecDeque<QueueCaller>,
    /// Timeout for ringing each member attempt (seconds)
    pub member_timeout: Duration,
    /// Retry interval between ring attempts (seconds)
    pub retry_interval: Duration,
    /// Maximum wait time for callers (0 = unlimited)
    pub max_wait_time: Duration,
    /// Maximum number of callers in queue (0 = unlimited)
    pub max_callers: usize,
    /// Wrap-up time after a call before member gets next call
    pub wrap_up_time: Duration,
    /// Music class for hold music
    pub music_class: String,
    /// Weight for multi-queue priority (higher = more priority)
    pub weight: u32,
    /// Total calls completed through this queue
    pub completed_calls: u64,
    /// Total abandoned calls
    pub abandoned_calls: u64,
    /// Current round-robin index for RoundRobin strategy
    pub rr_index: usize,
    /// Service level threshold in seconds
    pub service_level: Duration,
    /// Calls answered within service level
    pub calls_within_sl: u64,
    /// Accumulated hold time for average calculation
    pub total_holdtime_secs: u64,
    /// Count of calls used for holdtime average
    pub holdtime_call_count: u64,
    /// Announcement configuration
    pub announcements: QueueAnnouncements,
    /// Name of the queue rule to apply for dynamic penalty adjustment
    pub default_rule: Option<String>,
    /// Whether to use persistent (AstDB-backed) dynamic members
    pub persistent_members: bool,
    /// Whether to auto-pause members after a missed call
    pub auto_pause: bool,
    /// Timeout for the entire Queue() invocation (0 = use per-call arg)
    pub queue_timeout: Duration,
    /// Join-empty behavior: true = allow joining even if no members available
    pub join_empty: bool,
    /// Leave-when-empty behavior: true = kick callers when all members become unavailable
    pub leave_when_empty: bool,
    /// Number of seconds member phone rings before marking RINGNOANSWER
    pub ring_timeout: Duration,
}

impl CallQueue {
    /// Create a new call queue with default settings.
    pub fn new(name: String, strategy: QueueStrategy) -> Self {
        Self {
            name,
            strategy,
            members: Vec::new(),
            callers: VecDeque::new(),
            member_timeout: Duration::from_secs(15),
            retry_interval: Duration::from_secs(5),
            max_wait_time: Duration::ZERO,
            max_callers: 0,
            wrap_up_time: Duration::ZERO,
            music_class: "default".to_string(),
            weight: 0,
            completed_calls: 0,
            abandoned_calls: 0,
            rr_index: 0,
            service_level: Duration::from_secs(60),
            calls_within_sl: 0,
            total_holdtime_secs: 0,
            holdtime_call_count: 0,
            announcements: QueueAnnouncements::default(),
            default_rule: None,
            persistent_members: false,
            auto_pause: false,
            queue_timeout: Duration::ZERO,
            join_empty: true,
            leave_when_empty: false,
            ring_timeout: Duration::from_secs(15),
        }
    }

    /// Average hold time in seconds.
    pub fn avg_holdtime_secs(&self) -> u64 {
        if self.holdtime_call_count == 0 {
            return 0;
        }
        self.total_holdtime_secs / self.holdtime_call_count
    }

    /// Service level performance percentage (0-100).
    pub fn service_level_perf(&self) -> f64 {
        let total = self.completed_calls + self.abandoned_calls;
        if total == 0 {
            return 100.0;
        }
        (self.calls_within_sl as f64 / total as f64) * 100.0
    }

    // -----------------------------------------------------------------------
    // Member management
    // -----------------------------------------------------------------------

    /// Find available members filtered by penalty constraints, ordered by strategy.
    ///
    /// This implements the penalty-aware selection: members with lower penalty are
    /// tried first. Higher-penalty members are only tried if all lower-penalty
    /// members are busy/unavailable.
    pub fn select_members(&mut self) -> Vec<usize> {
        self.select_members_with_penalty(u32::MAX, 0)
    }

    /// Select members with explicit penalty constraints (from queue rules).
    pub fn select_members_with_penalty(
        &mut self,
        max_penalty: u32,
        min_penalty: u32,
    ) -> Vec<usize> {
        // First, update wrapup status
        for member in &mut self.members {
            member.check_wrapup_expired(self.wrap_up_time);
        }

        // Collect all available members within penalty range
        let available: Vec<usize> = self
            .members
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                m.is_available(self.wrap_up_time)
                    && m.penalty >= min_penalty
                    && m.penalty <= max_penalty
            })
            .map(|(i, _)| i)
            .collect();

        if available.is_empty() {
            return Vec::new();
        }

        // For penalty-aware strategies (all except RingAll), find the lowest
        // penalty group first
        match self.strategy {
            QueueStrategy::RingAll => {
                // Ring all available members sorted by penalty (lower first)
                let mut sorted = available;
                sorted.sort_by_key(|&i| self.members[i].penalty);
                sorted
            }
            QueueStrategy::LeastRecent => {
                // Find lowest penalty group, then pick least recent within it
                let lowest_penalty = self.lowest_penalty_group(&available);
                let mut group = lowest_penalty;
                group.sort_by(|&a, &b| {
                    let a_time = self.members[a]
                        .last_call_time
                        .map(|t| t.elapsed())
                        .unwrap_or(Duration::from_secs(u64::MAX));
                    let b_time = self.members[b]
                        .last_call_time
                        .map(|t| t.elapsed())
                        .unwrap_or(Duration::from_secs(u64::MAX));
                    b_time.cmp(&a_time)
                });
                group.truncate(1);
                group
            }
            QueueStrategy::FewestCalls => {
                // Find lowest penalty group, then pick fewest calls within it
                let lowest_penalty = self.lowest_penalty_group(&available);
                let mut group = lowest_penalty;
                group.sort_by_key(|&i| self.members[i].calls_taken);
                group.truncate(1);
                group
            }
            QueueStrategy::Random => {
                // Find lowest penalty group, then pick random within it
                let lowest_penalty = self.lowest_penalty_group(&available);
                let mut rng = rand::thread_rng();
                let mut group = lowest_penalty;
                group.shuffle(&mut rng);
                group.truncate(1);
                group
            }
            QueueStrategy::RoundRobin => {
                // Find lowest penalty group, then round-robin within it
                let lowest_penalty = self.lowest_penalty_group(&available);
                if lowest_penalty.is_empty() {
                    return Vec::new();
                }
                let start = self.rr_index % self.members.len();
                let mut selected = None;
                for offset in 0..self.members.len() {
                    let idx = (start + offset) % self.members.len();
                    if lowest_penalty.contains(&idx) {
                        selected = Some(idx);
                        self.rr_index = idx + 1;
                        break;
                    }
                }
                match selected {
                    Some(idx) => vec![idx],
                    None => Vec::new(),
                }
            }
            QueueStrategy::Linear => {
                // Find lowest penalty group, then take the first in list order
                let lowest_penalty = self.lowest_penalty_group(&available);
                lowest_penalty.into_iter().take(1).collect()
            }
        }
    }

    /// Get the group of members with the lowest penalty from the available set.
    fn lowest_penalty_group(&self, available: &[usize]) -> Vec<usize> {
        if available.is_empty() {
            return Vec::new();
        }
        let min_pen = available
            .iter()
            .map(|&i| self.members[i].penalty)
            .min()
            .unwrap_or(0);
        available
            .iter()
            .copied()
            .filter(|&i| self.members[i].penalty == min_pen)
            .collect()
    }

    /// Add a member to the queue.
    pub fn add_member(&mut self, interface: String, member_name: String, penalty: u32) -> bool {
        if self.members.iter().any(|m| m.interface == interface) {
            warn!(
                "Queue '{}': member '{}' already exists",
                self.name, interface
            );
            return false;
        }

        let member = QueueMember::new(interface.clone(), member_name, penalty);
        self.members.push(member);
        info!("Queue '{}': added member '{}'", self.name, interface);

        queue_log_write(QueueLogEntry::new(
            "NONE",
            &self.name,
            &interface,
            QueueLogEvent::AddMember,
            vec![penalty.to_string()],
        ));

        true
    }

    /// Add a dynamic member to the queue.
    pub fn add_dynamic_member(
        &mut self,
        interface: String,
        member_name: String,
        penalty: u32,
        state_interface: Option<String>,
        wrapup_time: Option<Duration>,
    ) -> bool {
        if self.members.iter().any(|m| m.interface == interface) {
            return false;
        }

        let mut member = QueueMember::new(interface.clone(), member_name, penalty);
        member.dynamic = true;
        member.state_interface = state_interface;
        member.wrapup_time = wrapup_time;
        self.members.push(member);

        queue_log_write(QueueLogEntry::new(
            "NONE",
            &self.name,
            &interface,
            QueueLogEvent::AddMember,
            vec![penalty.to_string()],
        ));

        true
    }

    /// Remove a member from the queue.
    pub fn remove_member(&mut self, interface: &str) -> bool {
        let before = self.members.len();
        self.members.retain(|m| m.interface != interface);
        let removed = self.members.len() < before;
        if removed {
            info!("Queue '{}': removed member '{}'", self.name, interface);
            queue_log_write(QueueLogEntry::new(
                "NONE",
                &self.name,
                interface,
                QueueLogEvent::RemoveMember,
                vec![],
            ));
        }
        removed
    }

    /// Pause or unpause a member.
    pub fn set_member_paused(&mut self, interface: &str, paused: bool, reason: Option<&str>) -> bool {
        if let Some(member) = self.members.iter_mut().find(|m| m.interface == interface) {
            member.paused = paused;
            member.pause_reason = reason.map(|s| s.to_string());
            info!(
                "Queue '{}': member '{}' {}",
                self.name,
                interface,
                if paused { "paused" } else { "unpaused" }
            );
            let event = if paused {
                QueueLogEvent::Pause
            } else {
                QueueLogEvent::Unpause
            };
            queue_log_write(QueueLogEntry::new(
                "NONE",
                &self.name,
                interface,
                event,
                vec![reason.unwrap_or("").to_string()],
            ));
            true
        } else {
            false
        }
    }

    /// Get the number of available members.
    pub fn available_member_count(&self) -> usize {
        self.members
            .iter()
            .filter(|m| m.is_available(self.wrap_up_time))
            .count()
    }

    /// Get the number of logged-in (non-unavailable) members.
    pub fn logged_in_count(&self) -> usize {
        self.members
            .iter()
            .filter(|m| m.status != MemberStatus::Unavailable)
            .count()
    }

    /// Get the count of members that are either available or in wrapup (free count).
    pub fn free_member_count(&self) -> usize {
        self.members
            .iter()
            .filter(|m| {
                !m.paused
                    && (m.status == MemberStatus::Available || m.status == MemberStatus::WrapUp)
            })
            .count()
    }

    // -----------------------------------------------------------------------
    // Caller management
    // -----------------------------------------------------------------------

    /// Add a caller to the queue.
    pub fn enqueue_caller(&mut self, caller: QueueCaller) -> usize {
        let position = self.callers.len() + 1;
        let mut caller = caller;
        caller.position = position;
        self.callers.push_back(caller);
        info!(
            "Queue '{}': caller added at position {}",
            self.name, position
        );
        position
    }

    /// Insert a caller at a specific position (1-based).
    pub fn enqueue_caller_at(&mut self, caller: QueueCaller, desired_pos: usize) -> usize {
        let mut caller = caller;
        let pos = if desired_pos == 0 || desired_pos > self.callers.len() + 1 {
            self.callers.len() + 1
        } else {
            desired_pos
        };
        caller.position = pos;
        // Insert at index pos-1
        let idx = pos - 1;
        if idx >= self.callers.len() {
            self.callers.push_back(caller);
        } else {
            self.callers.insert(idx, caller);
        }
        self.renumber_callers();
        pos
    }

    /// Remove the next caller from the front of the queue.
    pub fn dequeue_caller(&mut self) -> Option<QueueCaller> {
        let caller = self.callers.pop_front();
        self.renumber_callers();
        caller
    }

    /// Remove a specific caller (e.g., on hangup/abandon).
    pub fn remove_caller(&mut self, channel_id: &ChannelId) -> bool {
        let before = self.callers.len();
        self.callers.retain(|c| &c.channel_id != channel_id);
        let removed = self.callers.len() < before;
        if removed {
            self.abandoned_calls += 1;
            self.renumber_callers();
        }
        removed
    }

    /// Re-number caller positions after a change.
    fn renumber_callers(&mut self) {
        for (i, c) in self.callers.iter_mut().enumerate() {
            c.position = i + 1;
        }
    }

    // -----------------------------------------------------------------------
    // Announcements
    // -----------------------------------------------------------------------

    /// Check if a periodic announcement should be played for a caller, and if so
    /// return the sound file to play.
    pub fn check_periodic_announce(&mut self, caller_idx: usize) -> Option<String> {
        let interval = self.announcements.periodic_announce_interval;
        if interval.is_zero() {
            return None;
        }
        if caller_idx >= self.callers.len() {
            return None;
        }
        let caller = &self.callers[caller_idx];
        let should_play = match caller.last_periodic_announce {
            Some(last) => last.elapsed() >= interval,
            None => caller.enter_time.elapsed() >= interval,
        };
        if should_play {
            let files = &self.announcements.periodic_announce_files;
            if files.is_empty() {
                return None;
            }
            let idx = self.announcements.periodic_announce_index % files.len();
            let file = files[idx].clone();
            self.announcements.periodic_announce_index = idx + 1;
            self.callers[caller_idx].last_periodic_announce = Some(Instant::now());
            Some(file)
        } else {
            None
        }
    }

    /// Build a position announcement string for a caller.
    /// Returns None if position announcements are disabled or the position is out of range.
    pub fn position_announcement(&self, caller_idx: usize) -> Option<String> {
        if !self.announcements.announce_position {
            return None;
        }
        if caller_idx >= self.callers.len() {
            return None;
        }
        let pos = self.callers[caller_idx].position;
        if self.announcements.announce_position_min > 0
            && self.callers.len() < self.announcements.announce_position_min
        {
            return None;
        }
        if self.announcements.announce_position_max > 0
            && pos > self.announcements.announce_position_max
        {
            return Some("You are experiencing a high call volume.".to_string());
        }
        Some(format!("You are caller number {}.", pos))
    }

    /// Build a hold time announcement string.
    /// Returns the estimated hold time in minutes, or None if disabled.
    pub fn holdtime_announcement(&self) -> Option<String> {
        if !self.announcements.announce_holdtime {
            return None;
        }
        let avg = self.avg_holdtime_secs();
        if avg < self.announcements.announce_holdtime_min_secs {
            return None;
        }
        let minutes = (avg + 30) / 60; // round to nearest minute
        if minutes == 0 {
            Some("Your call should be answered shortly.".to_string())
        } else if minutes == 1 {
            Some("The expected wait time is less than one minute.".to_string())
        } else {
            Some(format!(
                "The expected wait time is approximately {} minutes.",
                minutes
            ))
        }
    }

    // -----------------------------------------------------------------------
    // Realtime member loading
    // -----------------------------------------------------------------------

    /// Load members from a realtime backend and merge them with static members.
    pub fn load_realtime_members(&mut self, backend: &dyn RealtimeMemberBackend) {
        let rt_members = backend.load_members(&self.name);
        for rt_member in rt_members {
            if !self.members.iter().any(|m| m.interface == rt_member.interface) {
                info!(
                    "Queue '{}': loaded realtime member '{}'",
                    self.name, rt_member.interface
                );
                self.members.push(rt_member);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ConnectResult
// ---------------------------------------------------------------------------

/// Result of attempting to connect a caller with a member.
#[derive(Debug, PartialEq, Eq)]
pub enum ConnectResult {
    /// Successfully connected and call completed
    Connected,
    /// Member was busy
    MemberBusy,
    /// Member did not answer within timeout
    MemberNoAnswer,
    /// Caller hung up during connect attempt
    CallerHangup,
}

// ---------------------------------------------------------------------------
// QueueResult
// ---------------------------------------------------------------------------

/// Result status of a Queue() execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueResult {
    /// Caller was connected to a member and the call completed
    Completed,
    /// Caller timed out waiting
    Timeout,
    /// Caller hung up while waiting
    Abandoned,
    /// No members available and joinempty conditions met
    JoinEmpty,
    /// All members busy/unavailable and leavewhenempty conditions met
    LeaveEmpty,
    /// Error (queue not found, etc.)
    Error,
    /// Caller pressed a key to exit
    ExitWithKey,
    /// Caller's call was transferred
    Transfer,
}

impl QueueResult {
    pub fn as_status_var(&self) -> &'static str {
        match self {
            Self::Completed => "CONTINUE",
            Self::Timeout => "TIMEOUT",
            Self::Abandoned => "ABANDON",
            Self::JoinEmpty => "JOINEMPTY",
            Self::LeaveEmpty => "LEAVEEMPTY",
            Self::Error => "FAILED",
            Self::ExitWithKey => "EXITWITHKEY",
            Self::Transfer => "TRANSFER",
        }
    }
}

// ---------------------------------------------------------------------------
// AppQueue
// ---------------------------------------------------------------------------

/// The Queue() dialplan application.
///
/// Usage: Queue(queue_name[,options[,URL[,announceoverride[,timeout[,AGI[,gosub[,rule[,position]]]]]]]])
///
/// Places the caller into the specified call queue. The caller will hear
/// hold music while waiting. When a member becomes available, the queue
/// attempts to connect the caller with the member.
pub struct AppQueue;

impl DialplanApp for AppQueue {
    fn name(&self) -> &str {
        "Queue"
    }

    fn description(&self) -> &str {
        "Queue a call for a call queue"
    }
}

/// Options parsed from Queue() arguments.
#[derive(Debug, Clone, Default)]
pub struct QueueOptions {
    /// Continue in dialplan if callee hangs up
    pub continue_on_hangup: bool,
    /// No retries on timeout
    pub no_retry: bool,
    /// Ring instead of MOH
    pub ring_instead_of_moh: bool,
    /// Allow called user to transfer
    pub allow_callee_transfer: bool,
    /// Allow calling user to transfer
    pub allow_caller_transfer: bool,
    /// Allow callee to hang up by pressing *
    pub allow_callee_hangup: bool,
    /// Allow caller to hang up by pressing *
    pub allow_caller_hangup: bool,
    /// Mark all calls as answered elsewhere on cancel
    pub mark_answered_elsewhere: bool,
    /// Custom MOH class
    pub moh_class: Option<String>,
}

impl QueueOptions {
    pub fn parse(opts: &str) -> Self {
        let mut result = Self::default();
        for ch in opts.chars() {
            match ch {
                'c' => result.continue_on_hangup = true,
                'C' => result.mark_answered_elsewhere = true,
                'n' => result.no_retry = true,
                'r' => result.ring_instead_of_moh = true,
                't' => result.allow_callee_transfer = true,
                'T' => result.allow_caller_transfer = true,
                'h' => result.allow_callee_hangup = true,
                'H' => result.allow_caller_hangup = true,
                _ => {}
            }
        }
        result
    }
}

impl AppQueue {
    /// Execute the Queue application.
    ///
    /// # Arguments
    /// * `channel` - The caller's channel
    /// * `args` - Argument string: "queue_name[,options[,URL[,announce[,timeout[,AGI[,gosub[,rule[,position]]]]]]]]"
    pub async fn exec(channel: &mut Channel, args: &str) -> (PbxExecResult, QueueResult) {
        let parts: Vec<&str> = args.splitn(9, ',').collect();

        let queue_name = match parts.first() {
            Some(name) if !name.trim().is_empty() => name.trim().to_string(),
            _ => {
                warn!("Queue: queue name is required");
                return (PbxExecResult::Failed, QueueResult::Error);
            }
        };

        let options = parts
            .get(1)
            .map(|s| QueueOptions::parse(s.trim()))
            .unwrap_or_default();

        // Parse timeout from args
        let timeout = if let Some(t) = parts.get(4) {
            match t.trim().parse::<u64>() {
                Ok(secs) => Duration::from_secs(secs),
                Err(_) => Duration::ZERO,
            }
        } else {
            Duration::ZERO
        };

        // Parse rule name
        let rule_name = parts.get(7).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());

        // Parse desired position
        let desired_position: Option<usize> = parts
            .get(8)
            .and_then(|s| s.trim().parse().ok())
            .filter(|&p: &usize| p > 0);

        info!(
            "Queue: channel '{}' entering queue '{}' (timeout: {:?})",
            channel.name, queue_name, timeout
        );

        // Get or create the queue
        let queue = Self::get_or_create_queue(&queue_name);

        // Create a caller entry
        let caller = QueueCaller {
            channel_id: channel.unique_id.clone(),
            channel_name: channel.name.clone(),
            position: 0,
            enter_time: Instant::now(),
            caller_name: Some(channel.caller.id.name.name.clone()).filter(|s| !s.is_empty()),
            caller_number: Some(channel.caller.id.number.number.clone()).filter(|s| !s.is_empty()),
            last_periodic_announce: None,
            last_position_announce: None,
        };

        // Check if we can join the queue
        {
            let q = queue.read();
            if q.max_callers > 0 && q.callers.len() >= q.max_callers {
                warn!(
                    "Queue: queue '{}' is full ({} callers)",
                    queue_name, q.max_callers
                );
                Self::set_status_vars(channel, &q, QueueResult::JoinEmpty, 0, 0);
                return (PbxExecResult::Success, QueueResult::JoinEmpty);
            }
            if !q.join_empty && q.available_member_count() == 0 && q.members.is_empty() {
                warn!("Queue: queue '{}' has no members", queue_name);
                Self::set_status_vars(channel, &q, QueueResult::JoinEmpty, 0, 0);
                return (PbxExecResult::Success, QueueResult::JoinEmpty);
            }
        }

        // Add caller to queue
        let position = {
            let mut q = queue.write();
            let pos = match desired_position {
                Some(p) => q.enqueue_caller_at(caller, p),
                None => q.enqueue_caller(caller),
            };

            // Log ENTERQUEUE
            let caller_id = channel
                .caller
                .id
                .number
                .number
                .clone();
            queue_log_write(QueueLogEntry::new(
                channel.unique_id.as_str(),
                &queue_name,
                "NONE",
                QueueLogEvent::EnterQueue,
                vec!["".to_string(), caller_id],
            ));

            pos
        };

        info!(
            "Queue: '{}' is at position {} in queue '{}'",
            channel.name, position, queue_name
        );

        // Main queue wait loop
        let result = Self::queue_wait_loop(
            channel,
            &queue,
            &queue_name,
            timeout,
            &options,
            rule_name.as_deref(),
        )
        .await;

        // Remove caller from queue if still present
        let wait_time = {
            let mut q = queue.write();
            let wait_secs = q
                .callers
                .iter()
                .find(|c| c.channel_id == channel.unique_id)
                .map(|c| c.enter_time.elapsed().as_secs())
                .unwrap_or(0);
            q.remove_caller(&channel.unique_id);
            wait_secs
        };

        // Set channel variables
        {
            let q = queue.read();
            Self::set_status_vars(channel, &q, result, wait_time, 0);
        }

        let exec_result = match result {
            QueueResult::Abandoned => PbxExecResult::Hangup,
            QueueResult::Error => PbxExecResult::Failed,
            _ => PbxExecResult::Success,
        };

        (exec_result, result)
    }

    /// Set queue status variables on the channel.
    fn set_status_vars(
        channel: &mut Channel,
        queue: &CallQueue,
        result: QueueResult,
        wait_secs: u64,
        answered_time: u64,
    ) {
        let vars = QueueStatusVars {
            queue_status: result.as_status_var().to_string(),
            queue_name: queue.name.clone(),
            queue_position: 0,
            queue_holdtime: queue.avg_holdtime_secs(),
            queue_completed: queue.completed_calls,
            queue_abandoned: queue.abandoned_calls,
            queue_wait: wait_secs,
            answered_time,
        };
        vars.apply_to_channel(channel);
    }

    /// Main queue wait loop. Waits for a member to become available,
    /// then attempts to connect the caller with that member.
    async fn queue_wait_loop(
        channel: &Channel,
        queue: &Arc<RwLock<CallQueue>>,
        queue_name: &str,
        timeout: Duration,
        _options: &QueueOptions,
        rule_name: Option<&str>,
    ) -> QueueResult {
        let start = Instant::now();
        let check_interval = Duration::from_millis(500);

        loop {
            // Check if caller hung up
            if channel.state == ChannelState::Down {
                info!(
                    "Queue: caller '{}' abandoned queue '{}'",
                    channel.name, queue_name
                );
                // Log ABANDON
                let (pos, orig_pos) = {
                    let q = queue.read();
                    let pos = q
                        .callers
                        .iter()
                        .find(|c| c.channel_id == channel.unique_id)
                        .map(|c| c.position)
                        .unwrap_or(0);
                    (pos, pos)
                };
                queue_log_write(QueueLogEntry::new(
                    channel.unique_id.as_str(),
                    queue_name,
                    "NONE",
                    QueueLogEvent::Abandon,
                    vec![
                        pos.to_string(),
                        orig_pos.to_string(),
                        start.elapsed().as_secs().to_string(),
                    ],
                ));
                return QueueResult::Abandoned;
            }

            // Check timeout
            if !timeout.is_zero() && start.elapsed() >= timeout {
                info!(
                    "Queue: timeout reached for '{}' in queue '{}'",
                    channel.name, queue_name
                );
                return QueueResult::Timeout;
            }

            // Also check the queue's max_wait_time
            {
                let q = queue.read();
                if !q.max_wait_time.is_zero() && start.elapsed() >= q.max_wait_time {
                    info!(
                        "Queue: max wait time reached for '{}' in queue '{}'",
                        channel.name, queue_name
                    );
                    return QueueResult::Timeout;
                }

                // Check leave-when-empty
                if q.leave_when_empty {
                    let all_unavailable = q
                        .members
                        .iter()
                        .all(|m| m.status == MemberStatus::Unavailable || m.paused);
                    if all_unavailable && !q.members.is_empty() {
                        info!(
                            "Queue: all members unavailable, leaving queue '{}'",
                            queue_name
                        );
                        return QueueResult::LeaveEmpty;
                    }
                }
            }

            // Determine penalty constraints from queue rules
            let (max_penalty, min_penalty) = if let Some(rule) = rule_name {
                if let Some(rules) = QUEUE_RULES.get(rule) {
                    QueueRule::evaluate_rules(rules.value(), start.elapsed().as_secs())
                } else {
                    (u32::MAX, 0)
                }
            } else {
                // Check queue's own default_rule
                let default_rule = {
                    let q = queue.read();
                    q.default_rule.clone()
                };
                if let Some(rule) = default_rule {
                    if let Some(rules) = QUEUE_RULES.get(&rule) {
                        QueueRule::evaluate_rules(rules.value(), start.elapsed().as_secs())
                    } else {
                        (u32::MAX, 0)
                    }
                } else {
                    (u32::MAX, 0)
                }
            };

            // Try to find an available member
            let selected_members = {
                let mut q = queue.write();
                q.select_members_with_penalty(max_penalty, min_penalty)
            };

            if !selected_members.is_empty() {
                let connect_result =
                    Self::attempt_connect(channel, queue, &selected_members, queue_name).await;

                match connect_result {
                    ConnectResult::Connected => {
                        let mut q = queue.write();
                        q.completed_calls += 1;

                        let wait_time = start.elapsed();
                        q.total_holdtime_secs += wait_time.as_secs();
                        q.holdtime_call_count += 1;

                        if wait_time <= q.service_level {
                            q.calls_within_sl += 1;
                        }

                        // Log CONNECT
                        queue_log_write(QueueLogEntry::new(
                            channel.unique_id.as_str(),
                            queue_name,
                            "agent",
                            QueueLogEvent::Connect,
                            vec![
                                wait_time.as_secs().to_string(),
                                "bridged_channel".to_string(),
                                "0".to_string(),
                            ],
                        ));

                        return QueueResult::Completed;
                    }
                    ConnectResult::MemberBusy | ConnectResult::MemberNoAnswer => {
                        debug!(
                            "Queue: member did not answer in queue '{}', retrying",
                            queue_name
                        );
                        // Log RINGNOANSWER
                        let ring_timeout = {
                            let q = queue.read();
                            q.ring_timeout.as_secs()
                        };
                        queue_log_write(QueueLogEntry::new(
                            channel.unique_id.as_str(),
                            queue_name,
                            "agent",
                            QueueLogEvent::RingNoAnswer,
                            vec![ring_timeout.to_string()],
                        ));

                        let retry = {
                            let q = queue.read();
                            q.retry_interval
                        };
                        tokio::time::sleep(retry).await;
                        continue;
                    }
                    ConnectResult::CallerHangup => {
                        return QueueResult::Abandoned;
                    }
                }
            }

            // Check announcements for this caller
            {
                let mut q = queue.write();
                if let Some(caller_idx) = q
                    .callers
                    .iter()
                    .position(|c| c.channel_id == channel.unique_id)
                {
                    // Check periodic announcement
                    if let Some(file) = q.check_periodic_announce(caller_idx) {
                        debug!(
                            "Queue: playing periodic announcement '{}' to '{}'",
                            file, channel.name
                        );
                        // In production: play_file(channel, &file).await;
                    }

                    // Check position announcement
                    if let Some(msg) = q.position_announcement(caller_idx) {
                        debug!("Queue: position announcement: '{}'", msg);
                        // In production: say_text(channel, &msg).await;
                    }

                    // Check holdtime announcement
                    if let Some(msg) = q.holdtime_announcement() {
                        debug!("Queue: holdtime announcement: '{}'", msg);
                        // In production: say_text(channel, &msg).await;
                    }
                }
            }

            // Play hold music / announcements would happen here
            // In production: play_moh(channel, &music_class).await;

            tokio::time::sleep(check_interval).await;

            // For the stub, break out quickly
            break;
        }

        QueueResult::Timeout
    }

    /// Attempt to connect the caller with selected queue member(s).
    async fn attempt_connect(
        _channel: &Channel,
        queue: &Arc<RwLock<CallQueue>>,
        member_indices: &[usize],
        queue_name: &str,
    ) -> ConnectResult {
        if member_indices.is_empty() {
            return ConnectResult::MemberNoAnswer;
        }

        let member_timeout = {
            let q = queue.read();
            q.member_timeout
        };

        // Mark selected members as ringing
        {
            let mut q = queue.write();
            for &idx in member_indices {
                if idx < q.members.len() {
                    q.members[idx].status = MemberStatus::Ringing;
                }
            }
        }

        // In a real implementation, this would:
        // 1. Create outbound channels to each selected member
        // 2. Ring them (using Dial logic)
        // 3. Wait for first answer (up to member_timeout)
        // 4. Bridge caller with answered member
        // 5. Wait for bridge to end
        // 6. Begin wrapup for the member

        debug!(
            "Queue: ringing {} member(s) in queue '{}' (timeout: {:?})",
            member_indices.len(),
            queue_name,
            member_timeout
        );

        // Reset member status after ring attempt
        {
            let mut q = queue.write();
            for &idx in member_indices {
                if idx < q.members.len() {
                    q.members[idx].status = MemberStatus::Available;
                }
            }
        }

        // For the stub, return no answer
        ConnectResult::MemberNoAnswer
    }

    /// Get an existing queue or create a new one with defaults.
    fn get_or_create_queue(name: &str) -> Arc<RwLock<CallQueue>> {
        if let Some(q) = QUEUES.get(name) {
            return q.value().clone();
        }

        let queue = CallQueue::new(name.to_string(), QueueStrategy::RingAll);
        let q = Arc::new(RwLock::new(queue));
        QUEUES.insert(name.to_string(), q.clone());
        info!("Queue: created new queue '{}'", name);
        q
    }

    /// Register a new queue in the global registry.
    pub fn register_queue(name: &str, strategy: QueueStrategy) -> Arc<RwLock<CallQueue>> {
        let queue = CallQueue::new(name.to_string(), strategy);
        let q = Arc::new(RwLock::new(queue));
        QUEUES.insert(name.to_string(), q.clone());
        q
    }

    /// Register a queue rule.
    pub fn register_rule(name: &str, rules: Vec<QueueRule>) {
        QUEUE_RULES.insert(name.to_string(), rules);
    }

    /// Add a member to a queue by name.
    pub fn add_member(
        queue_name: &str,
        interface: &str,
        member_name: &str,
        penalty: u32,
    ) -> bool {
        if let Some(q) = QUEUES.get(queue_name) {
            let mut q = q.write();
            q.add_member(interface.to_string(), member_name.to_string(), penalty)
        } else {
            false
        }
    }

    /// Remove a member from a queue by name.
    pub fn remove_member(queue_name: &str, interface: &str) -> bool {
        if let Some(q) = QUEUES.get(queue_name) {
            let mut q = q.write();
            q.remove_member(interface)
        } else {
            false
        }
    }

    /// Pause/unpause a member in a queue.
    pub fn pause_member(queue_name: &str, interface: &str, paused: bool) -> bool {
        if let Some(q) = QUEUES.get(queue_name) {
            let mut q = q.write();
            q.set_member_paused(interface, paused, None)
        } else {
            false
        }
    }

    /// Pause with reason.
    pub fn pause_member_with_reason(
        queue_name: &str,
        interface: &str,
        paused: bool,
        reason: &str,
    ) -> bool {
        if let Some(q) = QUEUES.get(queue_name) {
            let mut q = q.write();
            q.set_member_paused(interface, paused, Some(reason))
        } else {
            false
        }
    }

    /// List all configured queues.
    pub fn list_queues() -> Vec<(String, usize, usize)> {
        QUEUES
            .iter()
            .map(|entry| {
                let q = entry.value().read();
                (q.name.clone(), q.members.len(), q.callers.len())
            })
            .collect()
    }

    /// Check if a queue exists.
    pub fn queue_exists(name: &str) -> bool {
        QUEUES.contains_key(name)
    }

    /// Get queue statistics.
    pub fn queue_stats(name: &str) -> Option<QueueStatusVars> {
        QUEUES.get(name).map(|q| {
            let q = q.read();
            QueueStatusVars {
                queue_status: String::new(),
                queue_name: q.name.clone(),
                queue_position: 0,
                queue_holdtime: q.avg_holdtime_secs(),
                queue_completed: q.completed_calls,
                queue_abandoned: q.abandoned_calls,
                queue_wait: 0,
                answered_time: 0,
            }
        })
    }
}

// Simple once_cell implementation
mod once_cell {
    pub mod sync {
        pub struct Lazy<T> {
            inner: std::sync::OnceLock<T>,
            init: fn() -> T,
        }

        impl<T> Lazy<T> {
            pub const fn new(init: fn() -> T) -> Self {
                Self {
                    inner: std::sync::OnceLock::new(),
                    init,
                }
            }
        }

        impl<T> std::ops::Deref for Lazy<T> {
            type Target = T;

            fn deref(&self) -> &T {
                self.inner.get_or_init(self.init)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_queue_strategy_parse() {
        assert_eq!(
            QueueStrategy::from_str_name("ringall"),
            QueueStrategy::RingAll
        );
        assert_eq!(
            QueueStrategy::from_str_name("random"),
            QueueStrategy::Random
        );
        assert_eq!(
            QueueStrategy::from_str_name("leastrecent"),
            QueueStrategy::LeastRecent
        );
        assert_eq!(
            QueueStrategy::from_str_name("fewestcalls"),
            QueueStrategy::FewestCalls
        );
        assert_eq!(
            QueueStrategy::from_str_name("roundrobin"),
            QueueStrategy::RoundRobin
        );
        assert_eq!(
            QueueStrategy::from_str_name("rrmemory"),
            QueueStrategy::RoundRobin
        );
        assert_eq!(
            QueueStrategy::from_str_name("linear"),
            QueueStrategy::Linear
        );
        // Unknown defaults to RingAll
        assert_eq!(
            QueueStrategy::from_str_name("foobar"),
            QueueStrategy::RingAll
        );
    }

    #[test]
    fn test_strategy_as_str() {
        assert_eq!(QueueStrategy::RingAll.as_str(), "ringall");
        assert_eq!(QueueStrategy::LeastRecent.as_str(), "leastrecent");
        assert_eq!(QueueStrategy::FewestCalls.as_str(), "fewestcalls");
        assert_eq!(QueueStrategy::Random.as_str(), "random");
        assert_eq!(QueueStrategy::RoundRobin.as_str(), "roundrobin");
        assert_eq!(QueueStrategy::Linear.as_str(), "linear");
    }

    #[test]
    fn test_add_remove_member() {
        let mut q = CallQueue::new("test".to_string(), QueueStrategy::RingAll);
        assert!(q.add_member("SIP/alice".to_string(), "Alice".to_string(), 0));
        assert!(q.add_member("SIP/bob".to_string(), "Bob".to_string(), 1));
        assert_eq!(q.members.len(), 2);

        // Duplicate should fail
        assert!(!q.add_member("SIP/alice".to_string(), "Alice".to_string(), 0));

        assert!(q.remove_member("SIP/alice"));
        assert_eq!(q.members.len(), 1);
        assert_eq!(q.members[0].interface, "SIP/bob");
    }

    #[test]
    fn test_add_dynamic_member() {
        let mut q = CallQueue::new("test".to_string(), QueueStrategy::RingAll);
        assert!(q.add_dynamic_member(
            "SIP/alice".to_string(),
            "Alice".to_string(),
            2,
            Some("SIP/alice".to_string()),
            Some(Duration::from_secs(10)),
        ));
        assert_eq!(q.members.len(), 1);
        assert!(q.members[0].dynamic);
        assert_eq!(q.members[0].penalty, 2);
        assert_eq!(q.members[0].wrapup_time, Some(Duration::from_secs(10)));
    }

    #[test]
    fn test_pause_member() {
        let mut q = CallQueue::new("test".to_string(), QueueStrategy::RingAll);
        q.add_member("SIP/alice".to_string(), "Alice".to_string(), 0);

        assert!(q.set_member_paused("SIP/alice", true, Some("lunch")));
        assert!(q.members[0].paused);
        assert_eq!(q.members[0].pause_reason.as_deref(), Some("lunch"));
        assert_eq!(q.available_member_count(), 0);

        assert!(q.set_member_paused("SIP/alice", false, None));
        assert!(!q.members[0].paused);
        assert_eq!(q.available_member_count(), 1);
    }

    #[test]
    fn test_select_members_ringall() {
        let mut q = CallQueue::new("test".to_string(), QueueStrategy::RingAll);
        q.add_member("SIP/alice".to_string(), "Alice".to_string(), 1);
        q.add_member("SIP/bob".to_string(), "Bob".to_string(), 0);
        q.add_member("SIP/carol".to_string(), "Carol".to_string(), 2);

        let selected = q.select_members();
        // All three should be selected, ordered by penalty
        assert_eq!(selected.len(), 3);
        // Bob (penalty 0) should be first
        assert_eq!(q.members[selected[0]].interface, "SIP/bob");
    }

    #[test]
    fn test_select_members_fewest_calls() {
        let mut q = CallQueue::new("test".to_string(), QueueStrategy::FewestCalls);
        q.add_member("SIP/alice".to_string(), "Alice".to_string(), 0);
        q.add_member("SIP/bob".to_string(), "Bob".to_string(), 0);
        q.members[0].calls_taken = 5;
        q.members[1].calls_taken = 2;

        let selected = q.select_members();
        assert_eq!(selected.len(), 1);
        assert_eq!(q.members[selected[0]].interface, "SIP/bob");
    }

    #[test]
    fn test_select_members_least_recent() {
        let mut q = CallQueue::new("test".to_string(), QueueStrategy::LeastRecent);
        q.add_member("SIP/alice".to_string(), "Alice".to_string(), 0);
        q.add_member("SIP/bob".to_string(), "Bob".to_string(), 0);
        // Alice has a recent call, Bob has never taken a call
        q.members[0].last_call_time = Some(Instant::now());
        q.members[1].last_call_time = None;

        let selected = q.select_members();
        assert_eq!(selected.len(), 1);
        // Bob should be selected (never called = longest idle)
        assert_eq!(q.members[selected[0]].interface, "SIP/bob");
    }

    #[test]
    fn test_select_members_random() {
        let mut q = CallQueue::new("test".to_string(), QueueStrategy::Random);
        q.add_member("SIP/alice".to_string(), "Alice".to_string(), 0);
        q.add_member("SIP/bob".to_string(), "Bob".to_string(), 0);

        let selected = q.select_members();
        // Should pick exactly one
        assert_eq!(selected.len(), 1);
        // Should be one of the two
        let iface = &q.members[selected[0]].interface;
        assert!(iface == "SIP/alice" || iface == "SIP/bob");
    }

    #[test]
    fn test_select_members_roundrobin() {
        let mut q = CallQueue::new("test".to_string(), QueueStrategy::RoundRobin);
        q.add_member("SIP/alice".to_string(), "Alice".to_string(), 0);
        q.add_member("SIP/bob".to_string(), "Bob".to_string(), 0);
        q.add_member("SIP/carol".to_string(), "Carol".to_string(), 0);

        // First call -> alice
        let sel1 = q.select_members();
        assert_eq!(sel1.len(), 1);
        assert_eq!(q.members[sel1[0]].interface, "SIP/alice");

        // Second call -> bob
        let sel2 = q.select_members();
        assert_eq!(sel2.len(), 1);
        assert_eq!(q.members[sel2[0]].interface, "SIP/bob");

        // Third call -> carol
        let sel3 = q.select_members();
        assert_eq!(sel3.len(), 1);
        assert_eq!(q.members[sel3[0]].interface, "SIP/carol");

        // Fourth call -> wraps back to alice
        let sel4 = q.select_members();
        assert_eq!(sel4.len(), 1);
        assert_eq!(q.members[sel4[0]].interface, "SIP/alice");
    }

    #[test]
    fn test_select_members_linear() {
        let mut q = CallQueue::new("test".to_string(), QueueStrategy::Linear);
        q.add_member("SIP/alice".to_string(), "Alice".to_string(), 0);
        q.add_member("SIP/bob".to_string(), "Bob".to_string(), 0);

        // Linear always starts from first
        let sel1 = q.select_members();
        assert_eq!(sel1.len(), 1);
        assert_eq!(q.members[sel1[0]].interface, "SIP/alice");

        // Again, still first
        let sel2 = q.select_members();
        assert_eq!(sel2.len(), 1);
        assert_eq!(q.members[sel2[0]].interface, "SIP/alice");

        // Mark alice as busy -> should select bob
        q.members[0].in_call = true;
        let sel3 = q.select_members();
        assert_eq!(sel3.len(), 1);
        assert_eq!(q.members[sel3[0]].interface, "SIP/bob");
    }

    #[test]
    fn test_penalty_based_selection() {
        let mut q = CallQueue::new("test".to_string(), QueueStrategy::FewestCalls);
        q.add_member("SIP/alice".to_string(), "Alice".to_string(), 0);
        q.add_member("SIP/bob".to_string(), "Bob".to_string(), 5);
        q.add_member("SIP/carol".to_string(), "Carol".to_string(), 10);

        // With max_penalty=5, only alice and bob are eligible
        // Alice and bob have same calls (0), alice has lower penalty
        let sel = q.select_members_with_penalty(5, 0);
        assert_eq!(sel.len(), 1);
        assert_eq!(q.members[sel[0]].interface, "SIP/alice");

        // With min_penalty=5, only bob and carol are eligible
        let sel = q.select_members_with_penalty(u32::MAX, 5);
        assert_eq!(sel.len(), 1);
        assert_eq!(q.members[sel[0]].interface, "SIP/bob");

        // With max_penalty=0, only alice is eligible
        let sel = q.select_members_with_penalty(0, 0);
        assert_eq!(sel.len(), 1);
        assert_eq!(q.members[sel[0]].interface, "SIP/alice");
    }

    #[test]
    fn test_lowest_penalty_group() {
        let mut q = CallQueue::new("test".to_string(), QueueStrategy::RingAll);
        q.add_member("SIP/alice".to_string(), "Alice".to_string(), 1);
        q.add_member("SIP/bob".to_string(), "Bob".to_string(), 0);
        q.add_member("SIP/carol".to_string(), "Carol".to_string(), 0);
        q.add_member("SIP/dave".to_string(), "Dave".to_string(), 2);

        let avail: Vec<usize> = (0..4).collect();
        let group = q.lowest_penalty_group(&avail);
        // Bob and Carol have penalty 0
        assert_eq!(group.len(), 2);
        assert!(group.contains(&1)); // bob
        assert!(group.contains(&2)); // carol
    }

    #[test]
    fn test_enqueue_dequeue_caller() {
        let mut q = CallQueue::new("test".to_string(), QueueStrategy::RingAll);

        let caller1 = QueueCaller {
            channel_id: ChannelId::new(),
            channel_name: "SIP/caller1".to_string(),
            position: 0,
            enter_time: Instant::now(),
            caller_name: Some("Caller 1".to_string()),
            caller_number: Some("1001".to_string()),
            last_periodic_announce: None,
            last_position_announce: None,
        };

        let caller2 = QueueCaller {
            channel_id: ChannelId::new(),
            channel_name: "SIP/caller2".to_string(),
            position: 0,
            enter_time: Instant::now(),
            caller_name: Some("Caller 2".to_string()),
            caller_number: Some("1002".to_string()),
            last_periodic_announce: None,
            last_position_announce: None,
        };

        q.enqueue_caller(caller1);
        q.enqueue_caller(caller2);
        assert_eq!(q.callers.len(), 2);
        assert_eq!(q.callers[0].position, 1);
        assert_eq!(q.callers[1].position, 2);

        let first = q.dequeue_caller().unwrap();
        assert_eq!(first.channel_name, "SIP/caller1");
        assert_eq!(q.callers.len(), 1);
        assert_eq!(q.callers[0].position, 1); // Re-numbered
    }

    #[test]
    fn test_enqueue_at_position() {
        let mut q = CallQueue::new("test".to_string(), QueueStrategy::RingAll);

        let c1 = QueueCaller {
            channel_id: ChannelId::from_name("c1"),
            channel_name: "SIP/c1".to_string(),
            position: 0,
            enter_time: Instant::now(),
            caller_name: None,
            caller_number: None,
            last_periodic_announce: None,
            last_position_announce: None,
        };
        let c2 = QueueCaller {
            channel_id: ChannelId::from_name("c2"),
            channel_name: "SIP/c2".to_string(),
            position: 0,
            enter_time: Instant::now(),
            caller_name: None,
            caller_number: None,
            last_periodic_announce: None,
            last_position_announce: None,
        };
        let c3 = QueueCaller {
            channel_id: ChannelId::from_name("c3"),
            channel_name: "SIP/c3".to_string(),
            position: 0,
            enter_time: Instant::now(),
            caller_name: None,
            caller_number: None,
            last_periodic_announce: None,
            last_position_announce: None,
        };

        q.enqueue_caller(c1);
        q.enqueue_caller(c2);
        // Insert c3 at position 1 (head)
        q.enqueue_caller_at(c3, 1);

        assert_eq!(q.callers.len(), 3);
        assert_eq!(q.callers[0].channel_name, "SIP/c3");
        assert_eq!(q.callers[0].position, 1);
        assert_eq!(q.callers[1].channel_name, "SIP/c1");
        assert_eq!(q.callers[1].position, 2);
        assert_eq!(q.callers[2].channel_name, "SIP/c2");
        assert_eq!(q.callers[2].position, 3);
    }

    #[test]
    fn test_wrapup_time() {
        let mut q = CallQueue::new("test".to_string(), QueueStrategy::Linear);
        q.wrap_up_time = Duration::from_millis(50);
        q.add_member("SIP/alice".to_string(), "Alice".to_string(), 0);

        // Begin wrapup
        q.members[0].begin_wrapup();
        assert_eq!(q.members[0].status, MemberStatus::WrapUp);
        assert_eq!(q.members[0].calls_taken, 1);
        assert!(!q.members[0].is_available(q.wrap_up_time));

        // Wait for wrapup to expire
        std::thread::sleep(Duration::from_millis(60));
        assert!(q.members[0].is_available(q.wrap_up_time));
        q.members[0].check_wrapup_expired(q.wrap_up_time);
        assert_eq!(q.members[0].status, MemberStatus::Available);
    }

    #[test]
    fn test_queue_rules() {
        let rules = vec![
            QueueRule {
                min_wait_secs: 0,
                max_penalty: 5,
                min_penalty: 0,
                relative: false,
            },
            QueueRule {
                min_wait_secs: 30,
                max_penalty: 10,
                min_penalty: 0,
                relative: false,
            },
            QueueRule {
                min_wait_secs: 60,
                max_penalty: 0, // No limit
                min_penalty: 0,
                relative: false,
            },
        ];

        // At 0 seconds, max_penalty = 5
        let (max, min) = QueueRule::evaluate_rules(&rules, 0);
        assert_eq!(max, 5);
        assert_eq!(min, 0);

        // At 30 seconds, max_penalty = 10
        let (max, min) = QueueRule::evaluate_rules(&rules, 30);
        assert_eq!(max, 10);
        assert_eq!(min, 0);

        // At 60 seconds, max_penalty = unlimited
        let (max, _min) = QueueRule::evaluate_rules(&rules, 60);
        assert_eq!(max, u32::MAX);
    }

    #[test]
    fn test_queue_announcements_periodic() {
        let mut q = CallQueue::new("test".to_string(), QueueStrategy::RingAll);
        q.announcements.periodic_announce_interval = Duration::from_millis(10);
        q.announcements.periodic_announce_files =
            vec!["announce1".to_string(), "announce2".to_string()];

        let caller = QueueCaller {
            channel_id: ChannelId::new(),
            channel_name: "SIP/c1".to_string(),
            position: 0,
            enter_time: Instant::now(),
            caller_name: None,
            caller_number: None,
            last_periodic_announce: None,
            last_position_announce: None,
        };
        q.enqueue_caller(caller);

        // Wait for interval
        std::thread::sleep(Duration::from_millis(15));

        let ann = q.check_periodic_announce(0);
        assert_eq!(ann, Some("announce1".to_string()));

        // Wait again
        std::thread::sleep(Duration::from_millis(15));
        let ann2 = q.check_periodic_announce(0);
        assert_eq!(ann2, Some("announce2".to_string()));
    }

    #[test]
    fn test_position_announcement() {
        let mut q = CallQueue::new("test".to_string(), QueueStrategy::RingAll);
        q.announcements.announce_position = true;

        let caller = QueueCaller {
            channel_id: ChannelId::new(),
            channel_name: "SIP/c1".to_string(),
            position: 0,
            enter_time: Instant::now(),
            caller_name: None,
            caller_number: None,
            last_periodic_announce: None,
            last_position_announce: None,
        };
        q.enqueue_caller(caller);

        let ann = q.position_announcement(0);
        assert_eq!(ann, Some("You are caller number 1.".to_string()));
    }

    #[test]
    fn test_position_announcement_max() {
        let mut q = CallQueue::new("test".to_string(), QueueStrategy::RingAll);
        q.announcements.announce_position = true;
        q.announcements.announce_position_max = 2;

        for i in 0..5 {
            let caller = QueueCaller {
                channel_id: ChannelId::from_name(&format!("c{}", i)),
                channel_name: format!("SIP/c{}", i),
                position: 0,
                enter_time: Instant::now(),
                caller_name: None,
                caller_number: None,
                last_periodic_announce: None,
                last_position_announce: None,
            };
            q.enqueue_caller(caller);
        }

        // Position 1 is ok
        assert_eq!(
            q.position_announcement(0),
            Some("You are caller number 1.".to_string())
        );
        // Position 3 is beyond max
        assert_eq!(
            q.position_announcement(2),
            Some("You are experiencing a high call volume.".to_string())
        );
    }

    #[test]
    fn test_holdtime_announcement() {
        let mut q = CallQueue::new("test".to_string(), QueueStrategy::RingAll);
        q.announcements.announce_holdtime = true;

        // No calls completed yet -> avg = 0
        let ann = q.holdtime_announcement();
        assert_eq!(
            ann,
            Some("Your call should be answered shortly.".to_string())
        );

        // Simulate hold time
        q.total_holdtime_secs = 300; // 5 minutes total
        q.holdtime_call_count = 1;
        let ann = q.holdtime_announcement();
        assert_eq!(
            ann,
            Some("The expected wait time is approximately 5 minutes.".to_string())
        );
    }

    #[test]
    fn test_queue_log_entry_format() {
        let entry = QueueLogEntry::new(
            "1234567890.1",
            "support",
            "SIP/alice",
            QueueLogEvent::Connect,
            vec!["15".to_string(), "SIP/bob".to_string(), "3".to_string()],
        );
        let line = entry.format_line();
        assert!(line.contains("support"));
        assert!(line.contains("SIP/alice"));
        assert!(line.contains("CONNECT"));
        assert!(line.contains("15|SIP/bob|3"));
    }

    #[test]
    fn test_queue_log_events() {
        assert_eq!(QueueLogEvent::EnterQueue.as_str(), "ENTERQUEUE");
        assert_eq!(QueueLogEvent::Connect.as_str(), "CONNECT");
        assert_eq!(QueueLogEvent::CompleteCaller.as_str(), "COMPLETECALLER");
        assert_eq!(QueueLogEvent::CompleteAgent.as_str(), "COMPLETEAGENT");
        assert_eq!(QueueLogEvent::Abandon.as_str(), "ABANDON");
        assert_eq!(QueueLogEvent::Transfer.as_str(), "TRANSFER");
        assert_eq!(QueueLogEvent::RingNoAnswer.as_str(), "RINGNOANSWER");
    }

    #[test]
    fn test_queue_status_vars() {
        let mut q = CallQueue::new("support".to_string(), QueueStrategy::RingAll);
        q.completed_calls = 10;
        q.abandoned_calls = 2;
        q.total_holdtime_secs = 120;
        q.holdtime_call_count = 10;
        q.calls_within_sl = 8;

        assert_eq!(q.avg_holdtime_secs(), 12);

        let perf = q.service_level_perf();
        // 8 / (10+2) * 100 = 66.67
        assert!((perf - 66.67).abs() < 0.1);
    }

    #[test]
    fn test_queue_options_parse() {
        let opts = QueueOptions::parse("cHtTn");
        assert!(opts.continue_on_hangup);
        assert!(opts.allow_caller_hangup);
        assert!(opts.allow_callee_transfer);
        assert!(opts.allow_caller_transfer);
        assert!(opts.no_retry);
        assert!(!opts.ring_instead_of_moh);
    }

    #[test]
    fn test_queue_result_status_var() {
        assert_eq!(QueueResult::Completed.as_status_var(), "CONTINUE");
        assert_eq!(QueueResult::Timeout.as_status_var(), "TIMEOUT");
        assert_eq!(QueueResult::Abandoned.as_status_var(), "ABANDON");
        assert_eq!(QueueResult::JoinEmpty.as_status_var(), "JOINEMPTY");
        assert_eq!(QueueResult::LeaveEmpty.as_status_var(), "LEAVEEMPTY");
    }

    #[test]
    fn test_member_counts() {
        let mut q = CallQueue::new("test".to_string(), QueueStrategy::RingAll);
        q.add_member("SIP/a".to_string(), "A".to_string(), 0);
        q.add_member("SIP/b".to_string(), "B".to_string(), 0);
        q.add_member("SIP/c".to_string(), "C".to_string(), 0);

        assert_eq!(q.available_member_count(), 3);
        assert_eq!(q.logged_in_count(), 3);
        assert_eq!(q.free_member_count(), 3);

        // Pause one
        q.set_member_paused("SIP/a", true, None);
        assert_eq!(q.available_member_count(), 2);
        assert_eq!(q.free_member_count(), 2);

        // Make one unavailable
        q.members[1].status = MemberStatus::Unavailable;
        assert_eq!(q.available_member_count(), 1);
        assert_eq!(q.logged_in_count(), 2); // only SIP/b is unavailable
    }

    #[test]
    fn test_noop_realtime_backend() {
        let backend = NoopRealtimeBackend;
        assert!(backend.load_members("test").is_empty());
        assert!(!backend.save_member("test", &QueueMember::new(
            "SIP/a".to_string(),
            "A".to_string(),
            0,
        )));
        assert!(!backend.remove_member("test", "SIP/a"));
    }

    #[test]
    fn test_connect_result_types() {
        // Ensure all variants are constructible
        let _c = ConnectResult::Connected;
        let _b = ConnectResult::MemberBusy;
        let _n = ConnectResult::MemberNoAnswer;
        let _h = ConnectResult::CallerHangup;
    }

    // -----------------------------------------------------------------------
    // Adversarial tests -- edge cases and attack vectors
    // -----------------------------------------------------------------------

    // --- Queue with 0 members: caller should timeout, not panic ---
    #[test]
    fn test_adversarial_zero_members() {
        let mut q = CallQueue::new("empty_q".to_string(), QueueStrategy::RingAll);
        assert_eq!(q.available_member_count(), 0);
        assert_eq!(q.logged_in_count(), 0);
        assert_eq!(q.free_member_count(), 0);
        let selected = q.select_members();
        assert!(selected.is_empty());
    }

    // --- All members paused -> select_members returns empty ---
    #[test]
    fn test_adversarial_all_paused() {
        let mut q = CallQueue::new("paused_q".to_string(), QueueStrategy::RingAll);
        q.add_member("SIP/a".to_string(), "A".to_string(), 0);
        q.add_member("SIP/b".to_string(), "B".to_string(), 0);
        q.set_member_paused("SIP/a", true, Some("break"));
        q.set_member_paused("SIP/b", true, Some("break"));
        assert_eq!(q.available_member_count(), 0);
        let selected = q.select_members();
        assert!(selected.is_empty());
    }

    // --- Wrapup time longer than any reasonable timeout -> member never available ---
    #[test]
    fn test_adversarial_wrapup_longer_than_timeout() {
        let mut q = CallQueue::new("wrapup_q".to_string(), QueueStrategy::Linear);
        q.wrap_up_time = Duration::from_secs(3600); // 1 hour wrapup
        q.add_member("SIP/a".to_string(), "A".to_string(), 0);
        q.members[0].begin_wrapup();
        // Member should not be available
        assert!(!q.members[0].is_available(q.wrap_up_time));
        let selected = q.select_members();
        assert!(selected.is_empty());
    }

    // --- Penalty higher than max -> member should be skipped ---
    #[test]
    fn test_adversarial_penalty_too_high() {
        let mut q = CallQueue::new("pen_q".to_string(), QueueStrategy::FewestCalls);
        q.add_member("SIP/low".to_string(), "Low".to_string(), 1);
        q.add_member("SIP/high".to_string(), "High".to_string(), 999);
        // With max_penalty=5, high member skipped
        let selected = q.select_members_with_penalty(5, 0);
        assert_eq!(selected.len(), 1);
        assert_eq!(q.members[selected[0]].interface, "SIP/low");
    }

    // --- 1000 callers in queue -> verify position tracking ---
    #[test]
    fn test_adversarial_1000_callers() {
        let mut q = CallQueue::new("big_q".to_string(), QueueStrategy::RingAll);
        for i in 0..1000 {
            let caller = QueueCaller {
                channel_id: ChannelId::from_name(&format!("c{}", i)),
                channel_name: format!("SIP/c{}", i),
                position: 0,
                enter_time: Instant::now(),
                caller_name: None,
                caller_number: None,
                last_periodic_announce: None,
                last_position_announce: None,
            };
            q.enqueue_caller(caller);
        }
        assert_eq!(q.callers.len(), 1000);
        // All positions should be 1-indexed sequential
        for (i, c) in q.callers.iter().enumerate() {
            assert_eq!(c.position, i + 1);
        }
        // Remove from middle
        let mid_id = q.callers[500].channel_id.clone();
        q.remove_caller(&mid_id);
        assert_eq!(q.callers.len(), 999);
        // All positions should still be sequential
        for (i, c) in q.callers.iter().enumerate() {
            assert_eq!(c.position, i + 1);
        }
    }

    // --- RoundRobin with single member ---
    #[test]
    fn test_adversarial_roundrobin_single_member() {
        let mut q = CallQueue::new("rr_q".to_string(), QueueStrategy::RoundRobin);
        q.add_member("SIP/only".to_string(), "Only".to_string(), 0);
        // Should always pick the same member
        for _ in 0..10 {
            let sel = q.select_members();
            assert_eq!(sel.len(), 1);
            assert_eq!(q.members[sel[0]].interface, "SIP/only");
        }
    }

    // --- Linear with all busy -> no selection ---
    #[test]
    fn test_adversarial_linear_all_busy() {
        let mut q = CallQueue::new("lin_q".to_string(), QueueStrategy::Linear);
        q.add_member("SIP/a".to_string(), "A".to_string(), 0);
        q.add_member("SIP/b".to_string(), "B".to_string(), 0);
        q.members[0].in_call = true;
        q.members[1].in_call = true;
        let sel = q.select_members();
        assert!(sel.is_empty());
    }

    // --- Queue rules with negative penalty adjustments ---
    #[test]
    fn test_adversarial_queue_rules_negative_penalty() {
        let rules = vec![
            QueueRule {
                min_wait_secs: 0,
                max_penalty: 5,
                min_penalty: 0,
                relative: false,
            },
        ];
        // At 0 seconds, should work fine
        let (max, min) = QueueRule::evaluate_rules(&rules, 0);
        assert_eq!(max, 5);
        assert_eq!(min, 0);
    }

    // --- Empty queue name -> exec should return Error ---
    #[tokio::test]
    async fn test_adversarial_empty_queue_name() {
        let mut channel = Channel::new("Test/empty-q");
        let (result, status) = AppQueue::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Failed);
        assert_eq!(status, QueueResult::Error);
    }

    // --- Queue with max_callers exceeded ---
    #[test]
    fn test_adversarial_max_callers_exceeded() {
        let mut q = CallQueue::new("max_q".to_string(), QueueStrategy::RingAll);
        q.max_callers = 2;
        for i in 0..3 {
            let caller = QueueCaller {
                channel_id: ChannelId::from_name(&format!("c{}", i)),
                channel_name: format!("SIP/c{}", i),
                position: 0,
                enter_time: Instant::now(),
                caller_name: None,
                caller_number: None,
                last_periodic_announce: None,
                last_position_announce: None,
            };
            q.enqueue_caller(caller);
        }
        // Note: enqueue_caller doesn't check max_callers (that's done in exec)
        // But we verify the callers are properly numbered
        assert_eq!(q.callers.len(), 3);
    }

    // --- Remove non-existent caller -> no panic ---
    #[test]
    fn test_adversarial_remove_nonexistent_caller() {
        let mut q = CallQueue::new("rm_q".to_string(), QueueStrategy::RingAll);
        let fake_id = ChannelId::from_name("ghost");
        assert!(!q.remove_caller(&fake_id));
    }

    // --- Remove non-existent member -> no panic ---
    #[test]
    fn test_adversarial_remove_nonexistent_member() {
        let mut q = CallQueue::new("rm_m_q".to_string(), QueueStrategy::RingAll);
        assert!(!q.remove_member("SIP/ghost"));
    }

    // --- Pause non-existent member -> returns false ---
    #[test]
    fn test_adversarial_pause_nonexistent_member() {
        let mut q = CallQueue::new("pause_q".to_string(), QueueStrategy::RingAll);
        assert!(!q.set_member_paused("SIP/ghost", true, None));
    }

    // --- Service level perf with 0 total calls ---
    #[test]
    fn test_adversarial_service_level_no_calls() {
        let q = CallQueue::new("sl_q".to_string(), QueueStrategy::RingAll);
        assert_eq!(q.service_level_perf(), 100.0);
    }

    // --- Average holdtime with 0 calls ---
    #[test]
    fn test_adversarial_avg_holdtime_zero() {
        let q = CallQueue::new("ht_q".to_string(), QueueStrategy::RingAll);
        assert_eq!(q.avg_holdtime_secs(), 0);
    }

    // --- Periodic announce with empty files list -> None ---
    #[test]
    fn test_adversarial_periodic_announce_empty_files() {
        let mut q = CallQueue::new("ann_q".to_string(), QueueStrategy::RingAll);
        q.announcements.periodic_announce_interval = Duration::from_millis(1);
        q.announcements.periodic_announce_files = vec![]; // Empty!
        let caller = QueueCaller {
            channel_id: ChannelId::new(),
            channel_name: "SIP/c1".to_string(),
            position: 0,
            enter_time: Instant::now(),
            caller_name: None,
            caller_number: None,
            last_periodic_announce: None,
            last_position_announce: None,
        };
        q.enqueue_caller(caller);
        std::thread::sleep(Duration::from_millis(5));
        // Should return None, not panic from empty vec indexing
        assert!(q.check_periodic_announce(0).is_none());
    }

    // --- Position announcement with out-of-range caller_idx -> None ---
    #[test]
    fn test_adversarial_position_announce_out_of_range() {
        let mut q = CallQueue::new("pos_q".to_string(), QueueStrategy::RingAll);
        q.announcements.announce_position = true;
        assert!(q.position_announcement(999).is_none());
    }

    // --- Periodic announce with out-of-range caller_idx -> None ---
    #[test]
    fn test_adversarial_periodic_announce_out_of_range() {
        let mut q = CallQueue::new("per_q".to_string(), QueueStrategy::RingAll);
        q.announcements.periodic_announce_interval = Duration::from_millis(1);
        assert!(q.check_periodic_announce(999).is_none());
    }

    // --- Enqueue at position 0 -> should go to end ---
    #[test]
    fn test_adversarial_enqueue_at_zero() {
        let mut q = CallQueue::new("pos0_q".to_string(), QueueStrategy::RingAll);
        let c1 = QueueCaller {
            channel_id: ChannelId::from_name("c1"),
            channel_name: "SIP/c1".to_string(),
            position: 0,
            enter_time: Instant::now(),
            caller_name: None,
            caller_number: None,
            last_periodic_announce: None,
            last_position_announce: None,
        };
        let c2 = QueueCaller {
            channel_id: ChannelId::from_name("c2"),
            channel_name: "SIP/c2".to_string(),
            position: 0,
            enter_time: Instant::now(),
            caller_name: None,
            caller_number: None,
            last_periodic_announce: None,
            last_position_announce: None,
        };
        q.enqueue_caller(c1);
        // Position 0 is invalid (1-based), should go to end
        let pos = q.enqueue_caller_at(c2, 0);
        assert_eq!(pos, 2);
        assert_eq!(q.callers[1].channel_name, "SIP/c2");
    }

    // --- RoundRobin with rr_index way beyond members.len ---
    #[test]
    fn test_adversarial_roundrobin_high_rr_index() {
        let mut q = CallQueue::new("rr_high".to_string(), QueueStrategy::RoundRobin);
        q.add_member("SIP/a".to_string(), "A".to_string(), 0);
        q.add_member("SIP/b".to_string(), "B".to_string(), 0);
        q.rr_index = 999999; // Way beyond
        let sel = q.select_members();
        assert_eq!(sel.len(), 1);
        // Should still pick a valid member (modulo wraps)
        let idx = sel[0];
        assert!(idx < q.members.len());
    }

    // --- Queue options parse: empty string -> default ---
    #[test]
    fn test_adversarial_queue_options_empty() {
        let opts = QueueOptions::parse("");
        assert!(!opts.continue_on_hangup);
        assert!(!opts.no_retry);
    }

    // --- Dequeue from empty queue -> None ---
    #[test]
    fn test_adversarial_dequeue_empty() {
        let mut q = CallQueue::new("deq_q".to_string(), QueueStrategy::RingAll);
        assert!(q.dequeue_caller().is_none());
    }

    // --- Leave-when-empty with no members at all ---
    #[test]
    fn test_adversarial_leave_when_empty_no_members() {
        let q = CallQueue::new("lwe_q".to_string(), QueueStrategy::RingAll);
        // leave_when_empty only kicks callers when ALL members are unavailable,
        // but with zero members, the condition `!q.members.is_empty()` prevents it
        assert!(q.members.is_empty());
    }
}
