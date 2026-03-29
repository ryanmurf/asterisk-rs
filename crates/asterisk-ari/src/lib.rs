//! asterisk-ari: Asterisk REST Interface (ARI) implementation.
//!
//! This crate provides the full ARI HTTP/WebSocket server, including:
//! - REST resource handlers for channels, bridges, endpoints, recordings,
//!   playbacks, sounds, applications, device states, mailboxes, and the
//!   asterisk system resource.
//! - WebSocket event streaming to connected Stasis applications.
//! - Authentication, routing, and JSON serialization.

pub mod error;
pub mod models;
pub mod server;
pub mod websocket;
pub mod routes;
pub mod channels;
pub mod bridges;
pub mod endpoints;
pub mod recordings;
pub mod playbacks;
pub mod sounds;
pub mod applications;
pub mod asterisk_resource;
pub mod device_states;
pub mod mailboxes;
pub mod http_listener;

pub use error::{AriErrorKind, AriResult};
pub use models::*;
pub use server::{AriServer, AriConfig, AriAuth, AriRequest, AriResponse, HttpMethod, RestHandler};
pub use websocket::WebSocketSessionManager;
pub use applications::{StasisApp, StasisAppRegistry};
pub use http_listener::AriHttpListener;
