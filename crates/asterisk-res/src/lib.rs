//! asterisk-res: Core resource modules for Asterisk.
//!
//! Provides shared resource implementations used by channels and applications:
//!
//! - `srtp` - SRTP encryption for RTP (RFC 3711)
//! - `stun` - STUN client for NAT traversal (RFC 3489/5389)
//! - `http_server` - HTTP server for AMI/ARI web interfaces
//! - `musiconhold` - Music on Hold playback classes and player
//! - `parking` - Call parking lots and parked call management
//! - `agi` - Asterisk Gateway Interface (standard, FastAGI, AsyncAGI)
//! - `audiosocket` - AudioSocket protocol for external audio processing
//! - `speech` - Generic speech recognition engine framework
//! - `smdi` - Simplified Message Desk Interface (serial protocol)
//! - `config_options` - Typed configuration framework with validation
//! - `features` - In-call DTMF feature codes (transfer, disconnect, park)
//! - `cel` - Channel Event Logging with pluggable backends
//! - `astdb` - AstDB key-value store with file persistence
//! - `timing` - Timer abstraction with pthread-based implementation
//! - `fax` - FAX framework with pluggable technology engines
//! - `snmp` - SNMP agent with ASTERISK-MIB objects
//! - `rtp_multicast` - Multicast RTP for paging/intercom
//! - `media_cache` - HTTP media cache with freshness checking
//! - `nat` - NAT traversal helpers (symmetric RTP, rport, STUN)
//! - `t38` - T.38 fax over SIP (SDP attributes, re-INVITE)
//! - `path` - SIP Path header support (RFC 3327)
//! - `pidf` - PIDF presence XML generation (RFC 3863)
//! - `dialog_info` - Dialog-Info XML for BLF (RFC 4235)
//! - `endpoint_id` - SIP endpoint identification chain
//! - `phoneprov` - Phone provisioning with template expansion
//! - `limit` - System resource limits
//! - `security_log` - Security event logging
//! - `clioriginate` - CLI originate command
//! - `clialiases` - CLI command aliases
//! - `convert` - Audio format conversion CLI
//! - `mwi_external` - External MWI (Message Waiting Indicator)
//! - `sorcery` - Data access abstraction layer
//! - `prometheus` - Prometheus metrics exporter
//! - `statsd` - StatsD metrics client
//! - `xmpp` - XMPP/Jabber integration
//! - `calendar` - Calendar integration
//! - `config_curl` - cURL-based configuration backend
//! - `config_ldap` - LDAP configuration backend
//! - `config_odbc` - ODBC configuration backend
//! - `config_pgsql` - PostgreSQL configuration backend
//! - `config_sqlite3` - SQLite3 configuration backend
//! - `stasis_app` - Stasis application framework core
//! - `stasis_recording` - Recording management for Stasis
//! - `stasis_playback` - Playback management for Stasis
//! - `stasis_snoop` - Channel snooping via Stasis
//! - `stasis_device_state` - Custom device state via Stasis
//! - `stasis_mailbox` - Mailbox management via Stasis
//! - `stasis_answer` - Auto-answer for Stasis channels
//! - `realtime` - Realtime CLI commands
//! - `adsi` - ADSI (Analog Display Services Interface) support
//! - `ael` - AEL dialplan language parser
//! - `lua` - Lua dialplan scripting (stub)
//! - `format_attr` - Format attribute handlers (Opus, SILK, VP8, H264, H263)
//! - `speech_aeap` - AEAP speech engine interface
//! - `tonedetect` - Tone detection (Goertzel algorithm)
//! - `geolocation_res` - Geolocation framework (PIDF-LO, civic address, GML)

pub mod http_server;
pub mod srtp;
pub mod stun;
pub mod musiconhold;
pub mod parking;
pub mod agi;
pub mod audiosocket;
pub mod speech;
pub mod smdi;
pub mod config_options;
pub mod features;
pub mod cel;
pub mod astdb;
pub mod timing;
pub mod fax;
pub mod snmp;
pub mod rtp_multicast;
pub mod media_cache;
pub mod nat;
pub mod t38;
pub mod path;
pub mod pidf;
pub mod dialog_info;
pub mod endpoint_id;
pub mod phoneprov;
pub mod limit;
pub mod security_log;
pub mod clioriginate;
pub mod clialiases;
pub mod convert;
pub mod mwi_external;
pub mod sorcery;
pub mod prometheus;
pub mod statsd;
pub mod xmpp;
pub mod calendar;
pub mod config_curl;
pub mod config_ldap;
pub mod config_odbc;
pub mod config_pgsql;
pub mod config_sqlite3;
pub mod stasis_app;
pub mod stasis_recording;
pub mod stasis_playback;
pub mod stasis_snoop;
pub mod stasis_device_state;
pub mod stasis_mailbox;
pub mod stasis_answer;
pub mod realtime;
pub mod adsi;
pub mod ael;
pub mod lua;
pub mod format_attr;
pub mod speech_aeap;
pub mod tonedetect;
pub mod geolocation_res;
pub mod cel_beanstalkd;
pub mod cel_odbc;
pub mod cel_pgsql;
pub mod cel_radius;
pub mod cel_sqlite3;
pub mod cel_tds;
pub mod eagi;
pub mod dns_srv;

pub use http_server::HttpServer;
pub use srtp::SrtpSession;
pub use stun::StunClient;
pub use musiconhold::{MohClass, MohManager, MohMode, MohPlayer};
pub use parking::{ParkingLot, ParkingLotConfig, ParkingManager, ParkedCall};
pub use agi::{
    AgiCommandRegistry, AgiEnvironment, AgiMode, AgiResponse, AgiSession, FastAgiSession,
    FastAgiServer, FastAgiServerConfig, handle_agi_command, parse_agi_command,
};
pub use audiosocket::{AudioSocketConnection, AudioSocketKind, AudioSocketMessage};
pub use speech::{Speech, SpeechEngine, SpeechEngineRegistry, SpeechRecognitionResult, SpeechState};
pub use smdi::{SmdiInterface, SmdiMdMessage, SmdiMdType, SmdiMwiMessage};
pub use config_options::{ConfigOptionDef, ConfigOptionSet, ConfigOptionType, ConfigValue};
pub use features::{BuiltinFeature, ChannelFeatureOverrides, FeatureSet};
pub use cel::{CelBackend, CelEngine, CelEvent, CelEventType};
pub use astdb::AstDb;
pub use timing::{PthreadTiming, TimerEvent, TimerHandle, TimingInterface};
pub use fax::{FaxResult_, FaxSession, FaxState, FaxTechnology};
pub use snmp::{MibNode, MibValue, Oid, SnmpAgent};
pub use rtp_multicast::{MulticastRtp, MulticastType};
pub use media_cache::{CacheEntry, MediaCache};
pub use nat::{NatConfig, NatType, SymmetricRtpLearner, ViaInfo};
pub use t38::{T38Parameters, T38ReinviteRequest, T38State, T38UdpEc};
pub use path::{PathEntry, PathSet};
pub use pidf::{PidfBasicStatus, PidfDocument, PidfTuple};
pub use dialog_info::{DialogEntry, DialogInfo, DialogInfoState, DialogState};
pub use endpoint_id::{EndpointIdentifier, HeaderIdentifier, IdentifierChain, IdentifyContext, IpIdentifier, UsernameIdentifier};
pub use phoneprov::{PhoneProfile, PhoneProvManager, PhoneUser};
pub use limit::{ResourceLimit, ResourceType};
pub use security_log::{SecurityEvent, SecurityEventData, SecurityLogger};
pub use clioriginate::{OriginateRequest, OriginateTarget};
pub use clialiases::{CliAlias, CliAliasManager};
pub use convert::{ConvertRequest, ConvertStats};
pub use mwi_external::{ExternalMwi, MwiState};
