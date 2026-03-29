//! asterisk-funcs: Dialplan function implementations.
//!
//! This crate provides dialplan functions that can be used in expressions
//! within Asterisk dialplan configurations. Functions are evaluated at
//! runtime and can read/write channel properties and variables.
//!
//! Ported from Asterisk C funcs/:
//! - func_callerid.c -> FuncCallerId
//! - func_channel.c -> FuncChannel
//! - func_strings.c -> FuncStrings
//! - func_math.c -> FuncMath
//! - func_logic.c -> FuncLogic
//! - func_volume.c -> FuncVolume
//! - func_periodic_hook.c -> FuncPeriodicHook
//! - func_enum.c -> FuncEnumLookup, FuncEnumQuery, FuncEnumResult
//! - func_blacklist.c -> FuncBlacklist
//! - func_config.c -> FuncAstConfig
//! - func_jitterbuffer.c -> FuncJitterBuffer
//! - func_holdintercept.c -> FuncHoldIntercept
//! - func_talkdetect.c -> FuncTalkDetect
//! - func_pitchshift.c -> FuncPitchShift
//! - func_hash.c -> FuncHash, FuncHashKeys, FuncKeypadHash
//! - func_uri.c -> FuncUriEncode, FuncUriDecode
//! - func_base64.c -> FuncBase64Encode, FuncBase64Decode
//! - func_aes.c -> FuncAesEncrypt, FuncAesDecrypt
//! - func_json.c -> FuncJsonDecode, FuncJsonEncode
//! - func_sayfiles.c -> FuncSayFiles

pub mod callerid;
pub mod channel;
pub mod strings;
pub mod math;
pub mod logic;
pub mod global;
pub mod cdr;
pub mod registry;
pub mod timeout;
pub mod dialplan;
pub mod extstate;
pub mod groupcount;
pub mod db;
pub mod env;
pub mod curl;
pub mod shell;
pub mod sprintf;
pub mod rand;
pub mod volume;
pub mod periodic_hook;
pub mod enum_func;
pub mod blacklist;
pub mod config;
pub mod jitterbuf;
pub mod holdintercept;
pub mod talkdetect;
pub mod pitchshift;
pub mod hash;
pub mod uri;
pub mod base64_func;
pub mod aes;
pub mod json;
pub mod sayfiles;
pub mod connectedline;
pub mod redirecting;
pub mod speex_func;
pub mod lock;
pub mod cdr_func;
pub mod hangupcause;
pub mod talkdetect_func;
pub mod dialgroup;
pub mod enum_ext;
pub mod vmcount_ext;
pub mod sorcery;
pub mod presencestate;
pub mod talkdetect_ext;
pub mod srv;

pub use registry::FuncRegistry;

use std::collections::HashMap;

/// The result of evaluating a dialplan function.
pub type FuncResult = Result<String, FuncError>;

/// Errors that can occur when evaluating a dialplan function.
#[derive(Debug, Clone)]
pub enum FuncError {
    /// The function name is not registered
    UnknownFunction(String),
    /// Invalid argument(s) to the function
    InvalidArgument(String),
    /// The requested data type is not available
    DataNotAvailable(String),
    /// Write to a read-only function
    ReadOnly(String),
    /// Internal error
    Internal(String),
}

impl std::fmt::Display for FuncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownFunction(name) => write!(f, "Unknown function: {}", name),
            Self::InvalidArgument(msg) => write!(f, "Invalid argument: {}", msg),
            Self::DataNotAvailable(msg) => write!(f, "Data not available: {}", msg),
            Self::ReadOnly(name) => write!(f, "Function {} is read-only", name),
            Self::Internal(msg) => write!(f, "Internal error: {}", msg),
        }
    }
}

impl std::error::Error for FuncError {}

/// Context for evaluating dialplan functions.
///
/// This holds references to the channel and variable state needed
/// for function evaluation. In production, this would hold a reference
/// to the actual Channel object and the PBX variable store.
pub struct FuncContext {
    /// Channel variables (name -> value)
    pub variables: HashMap<String, String>,
    /// CallerID name
    pub caller_name: Option<String>,
    /// CallerID number
    pub caller_number: Option<String>,
    /// ANI
    pub ani: Option<String>,
    /// RDNIS (redirecting number)
    pub rdnis: Option<String>,
    /// DNID (dialed number)
    pub dnid: Option<String>,
    /// Channel name
    pub channel_name: String,
    /// Channel unique ID
    pub channel_uniqueid: String,
    /// Channel linked ID
    pub channel_linkedid: String,
    /// Channel state string
    pub channel_state: String,
    /// Account code
    pub account_code: String,
    /// Current context
    pub context: String,
    /// Current extension
    pub extension: String,
    /// Current priority
    pub priority: i32,
}

impl FuncContext {
    /// Create a new empty context.
    pub fn new() -> Self {
        Self {
            variables: HashMap::new(),
            caller_name: None,
            caller_number: None,
            ani: None,
            rdnis: None,
            dnid: None,
            channel_name: String::new(),
            channel_uniqueid: String::new(),
            channel_linkedid: String::new(),
            channel_state: "Down".to_string(),
            account_code: String::new(),
            context: "default".to_string(),
            extension: "s".to_string(),
            priority: 1,
        }
    }

    /// Create a context from a Channel.
    pub fn from_channel(channel: &asterisk_core::channel::Channel) -> Self {
        let caller_name = {
            let n = &channel.caller.id.name.name;
            if n.is_empty() { None } else { Some(n.clone()) }
        };
        let caller_number = {
            let n = &channel.caller.id.number.number;
            if n.is_empty() { None } else { Some(n.clone()) }
        };
        let ani = {
            let n = &channel.caller.ani.number.number;
            if n.is_empty() { None } else { Some(n.clone()) }
        };
        Self {
            variables: HashMap::new(),
            caller_name,
            caller_number,
            ani,
            rdnis: None,
            dnid: None,
            channel_name: channel.name.clone(),
            channel_uniqueid: channel.unique_id.as_str().to_string(),
            channel_linkedid: channel.linkedid.clone(),
            channel_state: channel.state.to_string(),
            account_code: channel.accountcode.clone(),
            context: channel.context.clone(),
            extension: channel.exten.clone(),
            priority: channel.priority,
        }
    }

    /// Set a variable.
    pub fn set_variable(&mut self, name: &str, value: &str) {
        self.variables.insert(name.to_string(), value.to_string());
    }

    /// Get a variable.
    pub fn get_variable(&self, name: &str) -> Option<&String> {
        self.variables.get(name)
    }
}

impl Default for FuncContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Trait for dialplan functions.
pub trait DialplanFunc: Send + Sync {
    /// The function name (e.g., "CALLERID", "CHANNEL", "LEN").
    fn name(&self) -> &str;

    /// Read the function value.
    fn read(&self, ctx: &FuncContext, args: &str) -> FuncResult;

    /// Write a value to the function (if supported).
    fn write(&self, _ctx: &mut FuncContext, _args: &str, _value: &str) -> Result<(), FuncError> {
        Err(FuncError::ReadOnly(self.name().to_string()))
    }
}
