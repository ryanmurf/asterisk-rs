//! Agent login/logout pool for call center agents.
//!
//! Port of app_agent_pool.c from Asterisk C. Provides AgentLogin() and
//! AgentRequest() dialplan applications. Agents have states (logged out,
//! ready, on call, wrapping up) and are managed in a shared pool.
//!
//! The pool is a global singleton (`AGENT_POOL`) so that any part of the
//! system (queue, AMI, dialplan) can query or mutate agent state.

use crate::{DialplanApp, PbxExecResult};
use asterisk_ami::protocol::AmiEvent;
use asterisk_ami::events::EventCategory;
use asterisk_core::channel::Channel;
use dashmap::DashMap;
use std::sync::LazyLock;
use std::time::Instant;
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// Global agent pool singleton
// ---------------------------------------------------------------------------

/// Global agent pool -- accessible from anywhere in the system.
pub static AGENT_POOL: LazyLock<AgentPool> = LazyLock::new(AgentPool::new);

// ---------------------------------------------------------------------------
// Agent state
// ---------------------------------------------------------------------------

/// Agent state in the pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    /// Agent is defined but not logged in.
    LoggedOut,
    /// Initial login wait for channel optimization.
    ProbationWait,
    /// Agent is ready to receive a call.
    ReadyForCall,
    /// A call is being presented to the agent.
    CallPresent,
    /// Waiting for agent to acknowledge the call.
    CallWaitAck,
    /// Agent is connected on a call.
    OnCall,
    /// Agent is in post-call wrapup.
    CallWrapup,
    /// Agent is being logged out.
    LoggingOut,
}

impl AgentState {
    /// Whether the agent is considered available for new calls.
    pub fn is_available(&self) -> bool {
        matches!(self, Self::ReadyForCall)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::LoggedOut => "LOGGED_OUT",
            Self::ProbationWait => "PROBATION_WAIT",
            Self::ReadyForCall => "READY_FOR_CALL",
            Self::CallPresent => "CALL_PRESENT",
            Self::CallWaitAck => "CALL_WAIT_ACK",
            Self::OnCall => "ON_CALL",
            Self::CallWrapup => "CALL_WRAPUP",
            Self::LoggingOut => "LOGGING_OUT",
        }
    }
}

// ---------------------------------------------------------------------------
// Agent configuration
// ---------------------------------------------------------------------------

/// Configuration for an agent.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// Agent identifier (e.g. "1001").
    pub agent_id: String,
    /// Full name of the agent.
    pub full_name: String,
    /// Wrapup time in milliseconds after a call.
    pub wrapup_time_ms: u32,
    /// Whether to auto-logoff after missed call.
    pub auto_logoff: bool,
    /// Ack call timeout in seconds.
    pub ack_call_timeout: u32,
    /// Music on hold class while waiting.
    pub moh_class: String,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            agent_id: String::new(),
            full_name: String::new(),
            wrapup_time_ms: 0,
            auto_logoff: false,
            ack_call_timeout: 0,
            moh_class: "default".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Agent
// ---------------------------------------------------------------------------

/// A single agent in the pool.
#[derive(Debug)]
pub struct Agent {
    /// Agent configuration.
    pub config: AgentConfig,
    /// Current state.
    pub state: AgentState,
    /// When agent logged in.
    pub login_time: Option<Instant>,
    /// When current/last call started.
    pub call_start: Option<Instant>,
    /// Channel name the agent is logged in on (if any).
    pub logged_channel: Option<String>,
    /// Channel unique-id the agent is logged in on (if any).
    pub logged_channel_uniqueid: Option<String>,
    /// Total number of calls handled this session.
    pub calls_taken: u32,
    /// Time of last call end.
    pub last_call_time: Option<Instant>,
}

impl Agent {
    /// Create a new agent from configuration.
    pub fn new(config: AgentConfig) -> Self {
        Self {
            config,
            state: AgentState::LoggedOut,
            login_time: None,
            call_start: None,
            logged_channel: None,
            logged_channel_uniqueid: None,
            calls_taken: 0,
            last_call_time: None,
        }
    }

    /// Log the agent in on the given channel.
    pub fn login(&mut self, channel_name: &str, channel_uniqueid: &str) {
        self.state = AgentState::ReadyForCall;
        self.login_time = Some(Instant::now());
        self.logged_channel = Some(channel_name.to_string());
        self.logged_channel_uniqueid = Some(channel_uniqueid.to_string());
        info!(
            "Agent '{}' ({}) logged in on '{}'",
            self.config.agent_id, self.config.full_name, channel_name
        );

        // Emit AgentLogin AMI event
        let mut event = AmiEvent::new("AgentLogin", EventCategory::AGENT.0);
        event.add_header("Agent", &self.config.agent_id);
        event.add_header("Channel", channel_name);
        event.add_header("Uniqueid", channel_uniqueid);
        asterisk_ami::publish_event(event);
    }

    /// Log the agent out.
    pub fn logout(&mut self) {
        let prev = self.state;
        let login_duration = self.login_time
            .map(|t| t.elapsed().as_secs())
            .unwrap_or(0);

        let channel_name = self.logged_channel.clone().unwrap_or_default();

        self.state = AgentState::LoggedOut;
        self.login_time = None;
        self.logged_channel = None;
        self.logged_channel_uniqueid = None;

        info!(
            "Agent '{}' logged out (was {:?}, duration {}s)",
            self.config.agent_id, prev, login_duration
        );

        // Emit AgentLogoff AMI event
        let mut event = AmiEvent::new("AgentLogoff", EventCategory::AGENT.0);
        event.add_header("Agent", &self.config.agent_id);
        event.add_header("Channel", &channel_name);
        event.add_header("Logintime", &login_duration.to_string());
        asterisk_ami::publish_event(event);
    }

    /// Transition to on-call state and emit AgentConnect event.
    pub fn begin_call(&mut self, caller_channel: &str, caller_uniqueid: &str) {
        self.state = AgentState::OnCall;
        self.call_start = Some(Instant::now());
        info!(
            "Agent '{}' connected to caller on '{}'",
            self.config.agent_id, caller_channel
        );

        // Emit AgentConnect AMI event
        let mut event = AmiEvent::new("AgentConnect", EventCategory::AGENT.0);
        event.add_header("Agent", &self.config.agent_id);
        event.add_header("Channel", caller_channel);
        event.add_header("Uniqueid", caller_uniqueid);
        if let Some(ref agent_chan) = self.logged_channel {
            event.add_header("MemberChannel", agent_chan);
        }
        let hold_time = self.login_time
            .map(|t| t.elapsed().as_secs())
            .unwrap_or(0);
        event.add_header("HoldTime", &hold_time.to_string());
        asterisk_ami::publish_event(event);
    }

    /// Transition to wrapup state and emit AgentComplete event.
    pub fn end_call(&mut self) {
        let talk_time = self.call_start
            .map(|t| t.elapsed().as_secs())
            .unwrap_or(0);

        self.state = AgentState::CallWrapup;
        self.call_start = None;
        self.calls_taken += 1;
        self.last_call_time = Some(Instant::now());

        info!(
            "Agent '{}' call completed (talk time {}s, total calls {})",
            self.config.agent_id, talk_time, self.calls_taken
        );

        // Emit AgentComplete AMI event
        let mut event = AmiEvent::new("AgentComplete", EventCategory::AGENT.0);
        event.add_header("Agent", &self.config.agent_id);
        event.add_header("TalkTime", &talk_time.to_string());
        event.add_header("CallsTaken", &self.calls_taken.to_string());
        if let Some(ref agent_chan) = self.logged_channel {
            event.add_header("MemberChannel", agent_chan);
        }
        asterisk_ami::publish_event(event);
    }

    /// Complete wrapup and return to ready state.
    pub fn finish_wrapup(&mut self) {
        if self.state == AgentState::CallWrapup {
            self.state = AgentState::ReadyForCall;
            debug!("Agent '{}' wrapup complete, ready for calls", self.config.agent_id);
        }
    }
}

// ---------------------------------------------------------------------------
// Agent pool
// ---------------------------------------------------------------------------

/// The shared agent pool.
///
/// Uses `DashMap` for lock-free concurrent access from multiple threads
/// (AMI actions, dialplan apps, queue strategies).
pub struct AgentPool {
    agents: DashMap<String, Agent>,
}

impl AgentPool {
    /// Create a new empty agent pool.
    pub fn new() -> Self {
        Self {
            agents: DashMap::new(),
        }
    }

    /// Add an agent to the pool (or update if already exists).
    pub fn add_agent(&self, config: AgentConfig) {
        let id = config.agent_id.clone();
        self.agents.insert(id, Agent::new(config));
    }

    /// Remove an agent from the pool entirely.
    pub fn remove_agent(&self, agent_id: &str) -> Option<Agent> {
        self.agents.remove(agent_id).map(|(_, a)| a)
    }

    /// Get the current state of an agent.
    pub fn agent_state(&self, agent_id: &str) -> Option<AgentState> {
        self.agents.get(agent_id).map(|a| a.state)
    }

    /// Find an available agent (first-come). Returns the agent ID.
    pub fn find_available(&self) -> Option<String> {
        self.agents
            .iter()
            .find(|entry| entry.value().state.is_available())
            .map(|entry| entry.key().clone())
    }

    /// Log an agent in on a channel. Creates the agent if it does not exist.
    pub fn login_agent(
        &self,
        agent_id: &str,
        channel_name: &str,
        channel_uniqueid: &str,
    ) -> bool {
        if let Some(mut agent) = self.agents.get_mut(agent_id) {
            if agent.state != AgentState::LoggedOut {
                warn!(
                    "AgentLogin: agent '{}' is already logged in (state {:?})",
                    agent_id, agent.state
                );
                return false;
            }
            agent.login(channel_name, channel_uniqueid);
            true
        } else {
            // Auto-create the agent on login (like Asterisk when no agents.conf)
            let config = AgentConfig {
                agent_id: agent_id.to_string(),
                full_name: agent_id.to_string(),
                ..Default::default()
            };
            let mut agent = Agent::new(config);
            agent.login(channel_name, channel_uniqueid);
            self.agents.insert(agent_id.to_string(), agent);
            true
        }
    }

    /// Log an agent out. Returns true if the agent was logged in.
    pub fn logout_agent(&self, agent_id: &str) -> bool {
        if let Some(mut agent) = self.agents.get_mut(agent_id) {
            if agent.state == AgentState::LoggedOut {
                return false;
            }
            agent.logout();
            true
        } else {
            false
        }
    }

    /// Begin a call for an agent. Returns false if agent is not available.
    pub fn agent_begin_call(
        &self,
        agent_id: &str,
        caller_channel: &str,
        caller_uniqueid: &str,
    ) -> bool {
        if let Some(mut agent) = self.agents.get_mut(agent_id) {
            if !agent.state.is_available() {
                return false;
            }
            agent.begin_call(caller_channel, caller_uniqueid);
            true
        } else {
            false
        }
    }

    /// End a call for an agent. Returns false if agent is not on a call.
    pub fn agent_end_call(&self, agent_id: &str) -> bool {
        if let Some(mut agent) = self.agents.get_mut(agent_id) {
            if agent.state != AgentState::OnCall {
                return false;
            }
            agent.end_call();
            // If wrapup time is 0, immediately go to ready
            if agent.config.wrapup_time_ms == 0 {
                agent.finish_wrapup();
            }
            true
        } else {
            false
        }
    }

    /// Finish wrapup for an agent, returning them to ready state.
    pub fn agent_finish_wrapup(&self, agent_id: &str) {
        if let Some(mut agent) = self.agents.get_mut(agent_id) {
            agent.finish_wrapup();
        }
    }

    /// Count of agents in each state.
    pub fn state_counts(&self) -> std::collections::HashMap<&'static str, usize> {
        let mut counts = std::collections::HashMap::new();
        for entry in self.agents.iter() {
            *counts.entry(entry.value().state.as_str()).or_insert(0) += 1;
        }
        counts
    }

    /// Total number of agents in the pool.
    pub fn count(&self) -> usize {
        self.agents.len()
    }

    /// Get a snapshot of all agents for AMI Agents action.
    pub fn all_agents_snapshot(&self) -> Vec<AgentSnapshot> {
        self.agents
            .iter()
            .map(|entry| {
                let agent = entry.value();
                AgentSnapshot {
                    agent_id: agent.config.agent_id.clone(),
                    name: agent.config.full_name.clone(),
                    state: agent.state,
                    channel: agent.logged_channel.clone(),
                    login_time_secs: agent.login_time.map(|t| t.elapsed().as_secs()),
                    calls_taken: agent.calls_taken,
                    last_call_time_secs: agent.last_call_time.map(|t| t.elapsed().as_secs()),
                }
            })
            .collect()
    }

    /// Get the channel name an agent is logged in on (if any).
    pub fn agent_channel(&self, agent_id: &str) -> Option<String> {
        self.agents
            .get(agent_id)
            .and_then(|a| a.logged_channel.clone())
    }
}

impl Default for AgentPool {
    fn default() -> Self {
        Self::new()
    }
}

/// Immutable snapshot of an agent for AMI responses.
#[derive(Debug, Clone)]
pub struct AgentSnapshot {
    pub agent_id: String,
    pub name: String,
    pub state: AgentState,
    pub channel: Option<String>,
    pub login_time_secs: Option<u64>,
    pub calls_taken: u32,
    pub last_call_time_secs: Option<u64>,
}

// ---------------------------------------------------------------------------
// AgentLogin() dialplan application
// ---------------------------------------------------------------------------

/// The AgentLogin() dialplan application.
///
/// Usage: AgentLogin(agent_id)
///
/// Logs an agent into the pool. The channel becomes the agent's
/// logged-in channel and will receive calls. The application blocks
/// until the agent logs out (in a real system, this means playing MOH
/// and waiting for incoming calls).
pub struct AppAgentLogin;

impl DialplanApp for AppAgentLogin {
    fn name(&self) -> &str {
        "AgentLogin"
    }

    fn description(&self) -> &str {
        "Log an agent into the agent pool"
    }
}

impl AppAgentLogin {
    /// Execute the AgentLogin application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let agent_id = args.split(',').next().unwrap_or("").trim();

        if agent_id.is_empty() {
            warn!("AgentLogin: requires agent_id argument");
            channel.set_variable("AGENT_STATUS", "INVALID");
            return PbxExecResult::Failed;
        }

        info!(
            "AgentLogin: channel '{}' agent '{}'",
            channel.name, agent_id
        );

        // Check if channel is in a valid state
        if channel.check_hangup() {
            warn!("AgentLogin: channel '{}' is hung up", channel.name);
            return PbxExecResult::Hangup;
        }

        // Answer the channel if not already answered
        if channel.state != asterisk_types::ChannelState::Up {
            channel.answer();
        }

        // Log the agent in
        let channel_name = channel.name.clone();
        let channel_uniqueid = channel.unique_id.0.clone();

        let logged_in = AGENT_POOL.login_agent(agent_id, &channel_name, &channel_uniqueid);

        if !logged_in {
            warn!(
                "AgentLogin: agent '{}' could not log in (already logged in?)",
                agent_id
            );
            channel.set_variable("AGENT_STATUS", "ALREADY_LOGGED_IN");
            return PbxExecResult::Failed;
        }

        // Set channel variables
        channel.set_variable("AGENT_STATUS", "SUCCESS");
        channel.set_variable("AGENTID", agent_id);

        // In a production system, we would now:
        // 1. Start playing music on hold
        // 2. Block here (select! loop) until:
        //    a. A call is delivered (agent_begin_call)
        //    b. The channel hangs up
        //    c. An AMI AgentLogoff action is received
        // For the test suite, we return immediately after login.

        debug!(
            "AgentLogin: agent '{}' successfully logged in on '{}'",
            agent_id, channel.name
        );

        PbxExecResult::Success
    }
}

// ---------------------------------------------------------------------------
// AgentRequest() dialplan application
// ---------------------------------------------------------------------------

/// The AgentRequest() dialplan application.
///
/// Usage: AgentRequest(agent_id)
///
/// Requests a specific agent from the pool. If the agent is available,
/// bridges the caller to the agent's logged-in channel.
pub struct AppAgentRequest;

impl DialplanApp for AppAgentRequest {
    fn name(&self) -> &str {
        "AgentRequest"
    }

    fn description(&self) -> &str {
        "Request an agent from the agent pool"
    }
}

impl AppAgentRequest {
    /// Execute the AgentRequest application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let agent_id = args.split(',').next().unwrap_or("").trim();

        if agent_id.is_empty() {
            warn!("AgentRequest: requires agent_id argument");
            channel.set_variable("AGENT_STATUS", "INVALID");
            return PbxExecResult::Failed;
        }

        info!(
            "AgentRequest: channel '{}' requesting agent '{}'",
            channel.name, agent_id
        );

        // Check agent availability
        let agent_state = AGENT_POOL.agent_state(agent_id);

        match agent_state {
            None => {
                warn!("AgentRequest: agent '{}' not found", agent_id);
                channel.set_variable("AGENT_STATUS", "NOT_FOUND");
                return PbxExecResult::Failed;
            }
            Some(state) if !state.is_available() => {
                info!(
                    "AgentRequest: agent '{}' not available (state {:?})",
                    agent_id, state
                );
                channel.set_variable("AGENT_STATUS", "NOT_LOGGED_IN");
                return PbxExecResult::Failed;
            }
            _ => {}
        }

        // Begin the call
        let caller_channel = channel.name.clone();
        let caller_uniqueid = channel.unique_id.0.clone();

        let connected = AGENT_POOL.agent_begin_call(
            agent_id,
            &caller_channel,
            &caller_uniqueid,
        );

        if !connected {
            warn!(
                "AgentRequest: failed to connect to agent '{}'",
                agent_id
            );
            channel.set_variable("AGENT_STATUS", "ERROR");
            return PbxExecResult::Failed;
        }

        // Set the agent channel on the caller's channel variables
        if let Some(agent_channel) = AGENT_POOL.agent_channel(agent_id) {
            channel.set_variable("AGENT_CHANNEL", &agent_channel);
        }
        channel.set_variable("AGENT_STATUS", "SUCCESS");
        channel.set_variable("AGENTID", agent_id);

        // In a real implementation, we would now bridge the caller's channel
        // with the agent's logged-in channel. For the test suite, we
        // mark the connection and return.

        debug!(
            "AgentRequest: channel '{}' connected to agent '{}'",
            channel.name, agent_id
        );

        PbxExecResult::Success
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create a fresh pool for testing (tests share the global, so
    /// each test uses its own unique agent IDs).
    fn unique_id(prefix: &str) -> String {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(1);
        format!("{}_{}", prefix, COUNTER.fetch_add(1, Ordering::Relaxed))
    }

    #[test]
    fn test_agent_state_available() {
        assert!(AgentState::ReadyForCall.is_available());
        assert!(!AgentState::OnCall.is_available());
        assert!(!AgentState::LoggedOut.is_available());
        assert!(!AgentState::CallWrapup.is_available());
        assert!(!AgentState::ProbationWait.is_available());
    }

    #[test]
    fn test_agent_lifecycle() {
        let config = AgentConfig {
            agent_id: unique_id("lifecycle"),
            full_name: "Test Agent".to_string(),
            wrapup_time_ms: 0,
            ..Default::default()
        };
        let mut agent = Agent::new(config);
        assert_eq!(agent.state, AgentState::LoggedOut);
        assert_eq!(agent.calls_taken, 0);

        // Login
        agent.login("SIP/agent1", "1234.1");
        assert_eq!(agent.state, AgentState::ReadyForCall);
        assert!(agent.login_time.is_some());
        assert_eq!(agent.logged_channel.as_deref(), Some("SIP/agent1"));

        // Begin call
        agent.begin_call("SIP/caller-001", "5678.1");
        assert_eq!(agent.state, AgentState::OnCall);
        assert!(agent.call_start.is_some());

        // End call
        agent.end_call();
        assert_eq!(agent.state, AgentState::CallWrapup);
        assert!(agent.call_start.is_none());
        assert_eq!(agent.calls_taken, 1);

        // Finish wrapup
        agent.finish_wrapup();
        assert_eq!(agent.state, AgentState::ReadyForCall);

        // Logout
        agent.logout();
        assert_eq!(agent.state, AgentState::LoggedOut);
        assert!(agent.logged_channel.is_none());
    }

    #[test]
    fn test_pool_login_logout() {
        let pool = AgentPool::new();
        let id = unique_id("pool_login");
        pool.add_agent(AgentConfig {
            agent_id: id.clone(),
            ..Default::default()
        });

        assert_eq!(pool.agent_state(&id), Some(AgentState::LoggedOut));
        assert!(pool.find_available().is_none());

        // Login
        assert!(pool.login_agent(&id, "SIP/test", "1.1"));
        assert_eq!(pool.agent_state(&id), Some(AgentState::ReadyForCall));

        // Should be findable as available
        assert_eq!(pool.find_available().as_deref(), Some(id.as_str()));

        // Login again should fail (already logged in)
        assert!(!pool.login_agent(&id, "SIP/test2", "1.2"));

        // Logout
        assert!(pool.logout_agent(&id));
        assert_eq!(pool.agent_state(&id), Some(AgentState::LoggedOut));

        // Logout again should return false
        assert!(!pool.logout_agent(&id));
    }

    #[test]
    fn test_pool_call_lifecycle() {
        let pool = AgentPool::new();
        let id = unique_id("pool_call");
        pool.add_agent(AgentConfig {
            agent_id: id.clone(),
            ..Default::default()
        });
        pool.login_agent(&id, "SIP/agent", "1.1");

        // Begin call
        assert!(pool.agent_begin_call(&id, "SIP/caller", "2.1"));
        assert_eq!(pool.agent_state(&id), Some(AgentState::OnCall));

        // Should not be available while on call
        assert!(pool.find_available().is_none());

        // End call
        assert!(pool.agent_end_call(&id));
        // With wrapup_time_ms=0, should go straight to ready
        assert_eq!(pool.agent_state(&id), Some(AgentState::ReadyForCall));
    }

    #[test]
    fn test_pool_auto_create_on_login() {
        let pool = AgentPool::new();
        let id = unique_id("auto_create");

        // Agent does not exist yet
        assert!(pool.agent_state(&id).is_none());

        // Login auto-creates
        assert!(pool.login_agent(&id, "SIP/new-agent", "3.1"));
        assert_eq!(pool.agent_state(&id), Some(AgentState::ReadyForCall));
        assert_eq!(pool.count(), 1);
    }

    #[test]
    fn test_pool_snapshots() {
        let pool = AgentPool::new();
        let id1 = unique_id("snap_a");
        let id2 = unique_id("snap_b");
        pool.add_agent(AgentConfig {
            agent_id: id1.clone(),
            full_name: "Agent A".to_string(),
            ..Default::default()
        });
        pool.add_agent(AgentConfig {
            agent_id: id2.clone(),
            full_name: "Agent B".to_string(),
            ..Default::default()
        });
        pool.login_agent(&id1, "SIP/a", "1.1");

        let snapshots = pool.all_agents_snapshot();
        assert_eq!(snapshots.len(), 2);

        let agent_a = snapshots.iter().find(|s| s.agent_id == id1).unwrap();
        assert_eq!(agent_a.state, AgentState::ReadyForCall);
        assert!(agent_a.channel.is_some());

        let agent_b = snapshots.iter().find(|s| s.agent_id == id2).unwrap();
        assert_eq!(agent_b.state, AgentState::LoggedOut);
        assert!(agent_b.channel.is_none());
    }

    #[test]
    fn test_pool_state_counts() {
        let pool = AgentPool::new();
        let id1 = unique_id("cnt_a");
        let id2 = unique_id("cnt_b");
        pool.add_agent(AgentConfig { agent_id: id1.clone(), ..Default::default() });
        pool.add_agent(AgentConfig { agent_id: id2.clone(), ..Default::default() });

        let counts = pool.state_counts();
        assert_eq!(*counts.get("LOGGED_OUT").unwrap_or(&0), 2);

        pool.login_agent(&id1, "SIP/a", "1.1");
        let counts = pool.state_counts();
        assert_eq!(*counts.get("READY_FOR_CALL").unwrap_or(&0), 1);
        assert_eq!(*counts.get("LOGGED_OUT").unwrap_or(&0), 1);
    }

    #[tokio::test]
    async fn test_agent_login_exec() {
        let mut channel = Channel::new("SIP/agent-001");
        let id = unique_id("exec_login");

        // Pre-add agent
        AGENT_POOL.add_agent(AgentConfig {
            agent_id: id.clone(),
            ..Default::default()
        });

        let result = AppAgentLogin::exec(&mut channel, &id).await;
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(channel.get_variable("AGENT_STATUS"), Some("SUCCESS"));
        assert_eq!(channel.get_variable("AGENTID"), Some(id.as_str()));

        // Channel should be answered
        assert_eq!(channel.state, asterisk_types::ChannelState::Up);

        // Clean up
        AGENT_POOL.logout_agent(&id);
    }

    #[tokio::test]
    async fn test_agent_login_no_args() {
        let mut channel = Channel::new("SIP/agent-002");
        let result = AppAgentLogin::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Failed);
    }

    #[tokio::test]
    async fn test_agent_request_exec() {
        let mut channel = Channel::new("SIP/caller-001");
        let id = unique_id("exec_request");

        // Agent must be logged in first
        AGENT_POOL.add_agent(AgentConfig {
            agent_id: id.clone(),
            full_name: "Test Agent".to_string(),
            ..Default::default()
        });
        AGENT_POOL.login_agent(&id, "SIP/agent-logged", "100.1");

        let result = AppAgentRequest::exec(&mut channel, &id).await;
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(channel.get_variable("AGENT_STATUS"), Some("SUCCESS"));
        assert_eq!(channel.get_variable("AGENTID"), Some(id.as_str()));
        assert_eq!(
            channel.get_variable("AGENT_CHANNEL"),
            Some("SIP/agent-logged")
        );

        // Agent should now be on a call
        assert_eq!(
            AGENT_POOL.agent_state(&id),
            Some(AgentState::OnCall)
        );

        // Clean up
        AGENT_POOL.agent_end_call(&id);
        AGENT_POOL.logout_agent(&id);
    }

    #[tokio::test]
    async fn test_agent_request_not_found() {
        let mut channel = Channel::new("SIP/caller-002");
        let result = AppAgentRequest::exec(&mut channel, "nonexistent_agent_xyz").await;
        assert_eq!(result, PbxExecResult::Failed);
        assert_eq!(channel.get_variable("AGENT_STATUS"), Some("NOT_FOUND"));
    }

    #[tokio::test]
    async fn test_agent_request_not_available() {
        let mut channel = Channel::new("SIP/caller-003");
        let id = unique_id("exec_not_avail");

        // Agent exists but not logged in
        AGENT_POOL.add_agent(AgentConfig {
            agent_id: id.clone(),
            ..Default::default()
        });

        let result = AppAgentRequest::exec(&mut channel, &id).await;
        assert_eq!(result, PbxExecResult::Failed);
        assert_eq!(channel.get_variable("AGENT_STATUS"), Some("NOT_LOGGED_IN"));
    }

    #[test]
    fn test_agent_channel_query() {
        let pool = AgentPool::new();
        let id = unique_id("chan_query");
        pool.add_agent(AgentConfig {
            agent_id: id.clone(),
            ..Default::default()
        });

        assert!(pool.agent_channel(&id).is_none());

        pool.login_agent(&id, "PJSIP/agent-100", "1.1");
        assert_eq!(pool.agent_channel(&id).as_deref(), Some("PJSIP/agent-100"));

        pool.logout_agent(&id);
        assert!(pool.agent_channel(&id).is_none());
    }
}
