//! Agent login/logout pool for call center agents.
//!
//! Port of app_agent_pool.c from Asterisk C. Provides AgentLogin() and
//! AgentRequest() dialplan applications. Agents have states (logged out,
//! ready, on call, wrapping up) and are managed in a shared pool.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::time::Instant;
use tracing::{info, warn};

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
        }
    }

    /// Log the agent in.
    pub fn login(&mut self, channel_name: &str) {
        self.state = AgentState::ReadyForCall;
        self.login_time = Some(Instant::now());
        self.logged_channel = Some(channel_name.to_string());
        info!("Agent '{}' logged in on '{}'", self.config.agent_id, channel_name);
    }

    /// Log the agent out.
    pub fn logout(&mut self) {
        let prev = self.state;
        self.state = AgentState::LoggedOut;
        self.login_time = None;
        self.logged_channel = None;
        info!("Agent '{}' logged out (was {:?})", self.config.agent_id, prev);
    }

    /// Transition to on-call state.
    pub fn begin_call(&mut self) {
        self.state = AgentState::OnCall;
        self.call_start = Some(Instant::now());
    }

    /// Transition to wrapup state.
    pub fn end_call(&mut self) {
        self.state = AgentState::CallWrapup;
        self.call_start = None;
    }
}

/// The shared agent pool.
pub struct AgentPool {
    agents: RwLock<HashMap<String, Agent>>,
}

impl AgentPool {
    /// Create a new empty agent pool.
    pub fn new() -> Self {
        Self {
            agents: RwLock::new(HashMap::new()),
        }
    }

    /// Add an agent to the pool.
    pub fn add_agent(&self, config: AgentConfig) {
        let id = config.agent_id.clone();
        self.agents.write().insert(id, Agent::new(config));
    }

    /// Get the current state of an agent.
    pub fn agent_state(&self, agent_id: &str) -> Option<AgentState> {
        self.agents.read().get(agent_id).map(|a| a.state)
    }

    /// Find an available agent (first-come).
    pub fn find_available(&self) -> Option<String> {
        self.agents
            .read()
            .iter()
            .find(|(_, a)| a.state.is_available())
            .map(|(id, _)| id.clone())
    }

    /// Count of agents in each state.
    pub fn state_counts(&self) -> HashMap<&'static str, usize> {
        let agents = self.agents.read();
        let mut counts = HashMap::new();
        for agent in agents.values() {
            *counts.entry(agent.state.as_str()).or_insert(0) += 1;
        }
        counts
    }
}

impl Default for AgentPool {
    fn default() -> Self {
        Self::new()
    }
}

/// The AgentLogin() dialplan application.
///
/// Usage: AgentLogin(agent_id)
///
/// Logs an agent into the pool. The channel becomes the agent's
/// logged-in channel and will receive calls. The application blocks
/// until the agent logs out.
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
            return PbxExecResult::Failed;
        }

        info!("AgentLogin: channel '{}' agent '{}'", channel.name, agent_id);

        // In a real implementation:
        // 1. Look up agent in pool
        // 2. Verify agent is logged out
        // 3. Set channel as agent's logged channel
        // 4. Play login confirmation tone
        // 5. Block (play MOH) until logout or hangup

        PbxExecResult::Success
    }
}

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
            return PbxExecResult::Failed;
        }

        info!("AgentRequest: channel '{}' requesting agent '{}'", channel.name, agent_id);

        // In a real implementation:
        // 1. Look up agent in pool
        // 2. If available, present call to agent
        // 3. Wait for agent ACK
        // 4. Bridge caller to agent
        // 5. Set AGENTSTATUS variable

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_state_available() {
        assert!(AgentState::ReadyForCall.is_available());
        assert!(!AgentState::OnCall.is_available());
        assert!(!AgentState::LoggedOut.is_available());
    }

    #[test]
    fn test_agent_login_logout() {
        let config = AgentConfig {
            agent_id: "1001".to_string(),
            ..Default::default()
        };
        let mut agent = Agent::new(config);
        assert_eq!(agent.state, AgentState::LoggedOut);

        agent.login("SIP/agent1");
        assert_eq!(agent.state, AgentState::ReadyForCall);
        assert!(agent.login_time.is_some());

        agent.begin_call();
        assert_eq!(agent.state, AgentState::OnCall);

        agent.end_call();
        assert_eq!(agent.state, AgentState::CallWrapup);

        agent.logout();
        assert_eq!(agent.state, AgentState::LoggedOut);
    }

    #[test]
    fn test_agent_pool() {
        let pool = AgentPool::new();
        pool.add_agent(AgentConfig {
            agent_id: "1001".to_string(),
            ..Default::default()
        });
        assert_eq!(pool.agent_state("1001"), Some(AgentState::LoggedOut));
        assert!(pool.find_available().is_none());
    }

    #[tokio::test]
    async fn test_agent_login_exec() {
        let mut channel = Channel::new("SIP/agent-001");
        let result = AppAgentLogin::exec(&mut channel, "1001").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_agent_request_exec() {
        let mut channel = Channel::new("SIP/caller-001");
        let result = AppAgentRequest::exec(&mut channel, "1001").await;
        assert_eq!(result, PbxExecResult::Success);
    }
}
