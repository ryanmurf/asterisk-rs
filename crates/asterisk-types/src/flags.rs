use bitflags::bitflags;

bitflags! {
    /// Channel flags used throughout Asterisk, matching various `AST_FLAG_*` defines.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct ChannelFlags: u64 {
        /// Defer DTMF processing
        const DEFER_DTMF = 1 << 0;
        /// Write interrupt in progress
        const WRITE_INT = 1 << 1;
        /// Channel is blocking
        const BLOCKING = 1 << 2;
        /// Disable device state event caching
        const DISABLE_DEVSTATE_CACHE = 1 << 3;
        /// Bridge hangup dont
        const BRIDGE_HANGUP_DONT = 1 << 4;
        /// Channel is outgoing
        const OUTGOING = 1 << 6;
        /// Channel is a zombie
        const ZOMBIE = 1 << 7;
        /// No optimize
        const NO_OPTIMIZE = 1 << 8;
        /// Channel is currently in a bridge
        const IN_BRIDGE = 1 << 9;
        /// Only process DTMF end frames
        const END_DTMF_ONLY = 1 << 10;
        /// Disable various workarounds
        const DISABLE_WORKAROUNDS = 1 << 11;
        /// Channel is being masqueraded
        const MASQ_NOSTREAM = 1 << 12;
        /// Channel is in auto-loop mode
        const IN_AUTOLOOP = 1 << 13;
        /// Outgoing channel for dialplan purposes
        const OUTGOING_LOGICAL = 1 << 14;
        /// Channel is dead / being destroyed
        const DEAD = 1 << 15;
    }
}

impl Default for ChannelFlags {
    fn default() -> Self {
        Self::empty()
    }
}

bitflags! {
    /// Bridge flags matching `AST_BRIDGE_FLAG_*` from bridge_features.h.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct BridgeFlags: u32 {
        /// Dissolve the bridge when a channel hangs up
        const DISSOLVE_HANGUP = 1 << 0;
        /// Dissolve the bridge when it becomes empty
        const DISSOLVE_EMPTY = 1 << 1;
        /// Smart bridge: automatically choose best technology
        const SMART = 1 << 2;
        /// Inhibit merge from this bridge
        const MERGE_INHIBIT_FROM = 1 << 3;
        /// Inhibit merge into this bridge
        const MERGE_INHIBIT_TO = 1 << 4;
        /// Inhibit swap from this bridge
        const SWAP_INHIBIT_FROM = 1 << 5;
        /// Inhibit swap into this bridge
        const SWAP_INHIBIT_TO = 1 << 6;
        /// Only masquerade operations allowed
        const MASQUERADE_ONLY = 1 << 7;
        /// Transfer is prohibited
        const TRANSFER_PROHIBITED = 1 << 8;
        /// Transfer bridge only
        const TRANSFER_BRIDGE_ONLY = 1 << 9;
        /// Bridge is invisible (not reported in events)
        const INVISIBLE = 1 << 10;
    }
}

impl Default for BridgeFlags {
    fn default() -> Self {
        Self::empty()
    }
}

bitflags! {
    /// Bridge capability flags matching `AST_BRIDGE_CAPABILITY_*` from bridge.h.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct BridgeCapability: u32 {
        /// Holding bridge capability
        const HOLDING = 1 << 0;
        /// Early bridge capability
        const EARLY = 1 << 1;
        /// Native bridge capability
        const NATIVE = 1 << 2;
        /// 1-to-1 mix bridge capability
        const ONE_TO_ONE_MIX = 1 << 3;
        /// Multi-party mix bridge capability
        const MULTI_MIX = 1 << 4;
    }
}

bitflags! {
    /// Frame flags matching `AST_FRFLAG_*` from frame.h.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct FrameFlags: u32 {
        /// Frame contains valid timing information
        const HAS_TIMING_INFO = 1 << 0;
        /// Frame has been requeued
        const REQUEUED = 1 << 1;
        /// Frame contains a valid sequence number
        const HAS_SEQUENCE_NUMBER = 1 << 2;
    }
}
