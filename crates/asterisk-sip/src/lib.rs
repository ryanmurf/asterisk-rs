//! asterisk-sip: SIP/RTP stack for the Asterisk Rust rewrite.
//!
//! Provides a complete SIP implementation including:
//! - SIP message parsing (RFC 3261)
//! - Transport layer (UDP/TCP) with IPv4/IPv6 dual-stack
//! - Transaction state machines
//! - Dialog management
//! - SDP offer/answer
//! - RTP media handling
//! - Digest authentication
//! - SIP channel driver
//! - Inbound REGISTER handling (registrar)
//! - Outbound REGISTER client (outbound registration)
//! - SUBSCRIBE/NOTIFY event framework (pubsub)
//! - SIP MESSAGE instant messaging
//! - SIP REFER call transfer
//! - Message Waiting Indicator (MWI)
//! - Arbitrary NOTIFY generation
//! - DTMF via SIP INFO
//! - Call diversion/forwarding (Diversion header)
//! - Access control lists (ACL)
//! - Digest authenticator (inbound + outbound)
//! - SDP/RTP integration
//! - Extended session handling (supplements, re-INVITE, session timers)
//! - History-Info header support
//! - Send-to-voicemail via SIP
//! - Caller ID from SIP headers (PAI, RPID)
//! - RFC 3326 Reason header for hangup cause
//! - Connected line updates via re-INVITE/UPDATE
//! - Empty SIP INFO handling
//! - Geolocation header (RFC 6442)
//! - Extension state NOTIFY for BLF
//! - SIP keepalive OPTIONS pings
//! - SIP message logging
//! - One-touch recording via SIP INFO
//! - PJSIP phone provisioning provider
//! - PJSIP configuration wizard
//! - GRUU (Globally Routable User Agent URI) — RFC 5627
//! - SIP Outbound — RFC 5626
//! - Multipart MIME body support
//! - Service-Route — RFC 3608
//! - Retry-After header parsing
//! - Early media fork handling
//! - Connection reuse & Via alias
//! - STIR/SHAKEN (RFC 8224/8225/8226) caller ID attestation & verification

pub mod parser;
pub mod transport;
pub mod transaction;
pub mod dialog;
pub mod session;
pub mod sdp;
pub mod auth;
pub mod rtp;
pub mod srtp;
pub mod crypto;
pub mod dtls;
pub mod ice;
pub mod stun;
pub mod turn;
pub mod stack;
pub mod channel_driver;
pub mod event_handler;
pub mod registrar;
pub mod outbound_registration;
pub mod pubsub;
pub mod messaging;
pub mod refer;
pub mod mwi;
pub mod notify;
pub mod notify_service;
pub mod dtmf_info;
pub mod diversion;
pub mod acl;
pub mod authenticator;
pub mod sdp_rtp;
pub mod session_ext;
pub mod history_info;
pub mod send_to_voicemail;
pub mod caller_id;
pub mod rfc3326;
pub mod connected_line;
pub mod empty_info;
pub mod geolocation;
pub mod exten_state;
pub mod keepalive;
pub mod logger;
pub mod one_touch_record;
pub mod phoneprov_pjsip;
pub mod config_wizard;
pub mod pjsip_config;
pub mod gruu;
pub mod outbound;
pub mod multipart;
pub mod service_route;
pub mod prack;
pub mod update;
pub mod stir_shaken;

pub use parser::{SipMessage, SipMethod, SipUri, StartLine};
pub use transport::SipTransport;
pub use transaction::{ClientTransaction, ServerTransaction};
pub use dialog::{Dialog, DialogState};
pub use session::SipSession;
pub use sdp::SessionDescription;
pub use auth::{DigestChallenge, DigestCredentials};
pub use rtp::{RtpSession, RtcpSession};
pub use channel_driver::SipChannelDriver;
pub use registrar::Registrar;
pub use outbound_registration::OutboundRegistration;
pub use pubsub::PubSub;
pub use messaging::InstantMessage;
pub use refer::TransferRequest;
pub use mwi::MwiState;
pub use acl::{Acl, AclRule, SipAcl};
pub use authenticator::{AuthCredentials, InboundAuthenticator, OutboundAuthenticator};
pub use sdp_rtp::RtpParameters;
pub use session_ext::{SessionSupplement, SupplementRegistry};
pub use stack::SipStack;
pub use event_handler::SipEventHandler;
pub use srtp::{SrtpCryptoSuite, SrtpKeyMaterial};
pub use gruu::Gruu;
pub use outbound::OutboundConfig;
pub use multipart::{MultipartBody, BodyPart};
pub use service_route::ServiceRouteSet;
pub use parser::RetryAfter;
pub use session::{EarlyMediaState, EarlyMediaConfig};
pub use ice::{IceAgent, IceCandidate, IceState, IceRole, IceMode, CandidateType, CandidatePair};
pub use stun::StunMessage;
pub use turn::TurnClient;
pub use rtp::ice_transport::IceRtpTransport;
pub use pjsip_config::{PjsipConfig, TransportConfig, EndpointConfig, AorConfig, AuthConfig, IdentifyConfig, RegistrationConfig, set_global_pjsip_config, get_global_pjsip_config};
pub use rtp::mos::{MosEstimator, CallQuality, QualityRating, CodecType, RtpMetrics};
pub use notify_service::{global_notify_service, NotifyService, ChannelSipState};
pub use stir_shaken::{
    AttestationLevel, StirIdentity, PASSporT, PASSporTHeader, PASSporTPayload,
    TelephoneNumber, VerificationResult, VerificationStatus, StirShakenError,
    CryptoBackend, HmacPlaceholderBackend, StubBackend, CertificateCache,
    StirShakenVars,
};

// Global SIP event handler, stored at startup so ConfBridge can send re-INVITEs.
use std::sync::{Arc, OnceLock};
static GLOBAL_EVENT_HANDLER: OnceLock<Arc<SipEventHandler>> = OnceLock::new();

/// Store the SIP event handler globally for use by ConfBridge SFU.
pub fn set_global_event_handler(handler: Arc<SipEventHandler>) {
    let _ = GLOBAL_EVENT_HANDLER.set(handler);
}

/// Retrieve the global SIP event handler.
pub fn get_global_event_handler() -> Option<Arc<SipEventHandler>> {
    GLOBAL_EVENT_HANDLER.get().cloned()
}

// Global broadcast for SIP call hangup events (BYE received).
// ConfBridge subscribes to detect when a remote UA hangs up.
static SIP_HANGUP_TX: std::sync::LazyLock<tokio::sync::broadcast::Sender<String>> =
    std::sync::LazyLock::new(|| tokio::sync::broadcast::channel(64).0);

/// Broadcast that a SIP call was hung up (BYE received). The payload is the SIP Call-ID.
pub fn notify_sip_hangup(call_id: &str) {
    let _ = SIP_HANGUP_TX.send(call_id.to_string());
}

/// Subscribe to SIP call hangup events.
pub fn subscribe_sip_hangup() -> tokio::sync::broadcast::Receiver<String> {
    SIP_HANGUP_TX.subscribe()
}
