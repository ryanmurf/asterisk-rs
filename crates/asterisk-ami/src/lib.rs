//! asterisk-ami: Asterisk Manager Interface (AMI) implementation.
//!
//! Port of main/manager.c from Asterisk C. The AMI is a TCP-based protocol
//! (default port 5038) that external programs use to manage and monitor
//! Asterisk. It provides:
//!
//! - Authentication (plaintext and MD5 challenge/response)
//! - Action dispatching (Login, Originate, Hangup, etc.)
//! - Event streaming (channel events, CDR events, etc.)
//! - Session management with event filters
//!
//! ## Protocol Format
//!
//! Actions (client -> server):
//! ```text
//! Action: Login\r\n
//! Username: admin\r\n
//! Secret: password\r\n
//! \r\n
//! ```
//!
//! Responses (server -> client):
//! ```text
//! Response: Success\r\n
//! Message: Authentication accepted\r\n
//! \r\n
//! ```
//!
//! Events (server -> client):
//! ```text
//! Event: Newchannel\r\n
//! Channel: SIP/1234-00000001\r\n
//! \r\n
//! ```

pub mod protocol;
pub mod session;
pub mod server;
pub mod actions;
pub mod events;
pub mod auth;
pub mod event_bus;

pub use protocol::{AmiAction, AmiResponse, AmiEvent};
pub use session::AmiSession;
pub use server::AmiServer;
pub use events::EventCategory;
pub use auth::AmiUser;
pub use event_bus::{AMI_EVENT_BUS, publish_event};
