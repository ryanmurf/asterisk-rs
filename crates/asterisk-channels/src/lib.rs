//! asterisk-channels: Channel driver implementations.
//!
//! This crate provides concrete channel technology drivers, each implementing
//! the `ChannelDriver` trait from `asterisk-core`. Ports of:
//!
//! - `core_local.c` -> `local` module (Local paired channels)
//! - `chan_audiosocket.c` -> `audiosocket` module (TCP-based external audio)
//! - `chan_rtp.c` -> `rtp_channel` module (bare RTP media channels)
//! - `chan_pjsip.c` -> `pjsip_channel` module (PJSIP SIP channel driver)
//! - `chan_bridge_media.c` -> `bridge_media` module (Bridge media channels)
//! - `chan_iax2.c` -> `iax2` module (IAX2 protocol channel driver)
//! - `chan_dahdi.c` -> `dahdi` module (DAHDI telephony interface)
//! - `chan_websocket.c` -> `websocket` module (WebSocket channels)

pub mod local;
pub mod audiosocket;
pub mod rtp_channel;
pub mod pjsip_channel;
pub mod bridge_media;
pub mod iax2;
pub mod dahdi;
pub mod websocket;
pub mod console;
pub mod phone;
pub mod unistim;
pub mod motif;
pub mod mgcp;
pub mod skinny;

pub use local::LocalChannelDriver;
pub use audiosocket::AudioSocketDriver;
pub use rtp_channel::RtpChannelDriver;
pub use pjsip_channel::PjsipChannelDriver;
pub use bridge_media::BridgeMediaDriver;
pub use iax2::Iax2Driver;
pub use dahdi::DahdiDriver;
pub use websocket::WebSocketDriver;
pub use console::ConsoleDriver;
pub use phone::PhoneDriver;
pub use unistim::UnistimDriver;
pub use motif::MotifDriver;
pub use mgcp::MgcpDriver;
pub use skinny::SkinnyDriver;
