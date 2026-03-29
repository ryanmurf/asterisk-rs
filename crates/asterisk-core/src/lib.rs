//! asterisk-core: Core PBX engine -- channels, bridges, dialplan, stasis, modules.
//!
//! This crate provides the central abstractions of the Asterisk PBX engine
//! rewritten in Rust, including the channel model, bridge framework, dialplan
//! execution engine (PBX), the Stasis message bus, task processors, scheduler,
//! and module system.

pub mod stasis;
pub mod channel;
pub mod frame;
pub mod bridge;
pub mod pbx;
pub mod taskprocessor;
pub mod scheduler;
pub mod module;

// Re-exports for convenience
pub use channel::{Channel, ChannelDriver, ChannelId, ChannelSnapshot, HangupCallback, register_hangup_callback, ChannelEventPublisher, register_channel_event_publisher};
pub use channel::store as channel_store;
pub use channel::softhangup;
pub use channel::dtmf;
pub use channel::generator;
pub use channel::audiohook;
pub use channel::framehook;
pub use channel::readwrite;
pub use channel::tech_registry::{ChannelTechRegistry, TECH_REGISTRY};
pub use bridge::{Bridge, BridgeChannel, BridgeSnapshot, BridgeTechnology, VideoMode};
pub use bridge::{bridge_create, bridge_join, bridge_leave, bridge_dissolve};
pub use bridge::{find_bridge, list_bridges, bridge_count};
pub use bridge::builtin_features::BuiltinFeatures;
pub use bridge::native_rtp::NativeRtpBridge;
pub use bridge::bridge_channel::BridgeChannelOps;
pub use bridge::event_loop::{bridge_channel_run, process_frame, should_pass_frame};
pub use bridge::softmix::SoftmixBridgeTech;
pub use bridge::basic::{BasicBridge, ast_bridge_basic_new};
pub use pbx::{
    Context, Dialplan, DialplanApp, DialplanFunction, Extension, PbxResult, Priority,
    set_global_dialplan, get_global_dialplan,
};
pub use pbx::app_registry::{AppRegistry, APP_REGISTRY};
pub use pbx::func_registry::{FuncRegistry, FUNC_REGISTRY};
pub use pbx::expression::evaluate_expression;
pub use pbx::substitute::{substitute_variables, substitute_variables_full};
pub use pbx::exec::{pbx_run, PbxRunResult};
pub use pbx::hints::{ExtensionState, HintRegistry, HINT_REGISTRY};
pub use stasis::{StasisCache, StasisMessage, Subscription, Topic};
pub use taskprocessor::TaskProcessor;
pub use scheduler::{SchedId, Scheduler};
pub use module::{Module, ModuleLoadResult, ModuleRegistry};
