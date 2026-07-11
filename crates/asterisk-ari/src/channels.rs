//! /ari/channels resource -- channel operations via the ARI REST interface.
//!
//! Port of res/ari/resource_channels.c. Implements all channel-related
//! ARI endpoints: list, originate, get, hangup, answer, ring, DTMF,
//! mute, hold, play, record, variable get/set, snoop, dial, and
//! externalMedia.

use crate::error::AriErrorKind;
use crate::models::*;
use crate::server::{AriRequest, AriResponse, AriServer, HttpMethod, RestHandler};
use asterisk_core::channel::tech_registry::TECH_REGISTRY;
use asterisk_core::channel_store;
use std::sync::Arc;

/// Build the /channels route subtree.
pub fn build_channels_routes() -> Arc<RestHandler> {
    let _channels = Arc::new(
        RestHandler::new("channels")
            .on(HttpMethod::Get, handle_list)
            .on(HttpMethod::Post, handle_originate),
    );

    // /channels/create
    let create = Arc::new(
        RestHandler::new("create").on(HttpMethod::Post, handle_create),
    );

    // /channels/externalMedia
    //
    // NOTE: exact-match children must be registered before the {channelId}
    // wildcard child -- route matching walks children in insertion order and
    // the wildcard matches any segment.
    let external_media = Arc::new(
        RestHandler::new("externalMedia").on(HttpMethod::Post, handle_external_media),
    );

    // /channels/{channelId}
    let _channel_by_id = Arc::new(
        RestHandler::new("{channelId}")
            .on(HttpMethod::Get, handle_get)
            .on(HttpMethod::Post, handle_originate_with_id)
            .on(HttpMethod::Delete, handle_hangup),
    );

    // /channels/{channelId}/continue
    let continue_handler = Arc::new(
        RestHandler::new("continue").on(HttpMethod::Post, handle_continue),
    );

    // /channels/{channelId}/redirect
    let redirect = Arc::new(
        RestHandler::new("redirect").on(HttpMethod::Post, handle_redirect),
    );

    // /channels/{channelId}/answer
    let answer = Arc::new(
        RestHandler::new("answer").on(HttpMethod::Post, handle_answer),
    );

    // /channels/{channelId}/ring
    let ring = Arc::new(
        RestHandler::new("ring")
            .on(HttpMethod::Post, handle_start_ring)
            .on(HttpMethod::Delete, handle_stop_ring),
    );

    // /channels/{channelId}/dtmf
    let dtmf = Arc::new(
        RestHandler::new("dtmf").on(HttpMethod::Post, handle_send_dtmf),
    );

    // /channels/{channelId}/mute
    let mute = Arc::new(
        RestHandler::new("mute")
            .on(HttpMethod::Post, handle_mute)
            .on(HttpMethod::Delete, handle_unmute),
    );

    // /channels/{channelId}/hold
    let hold = Arc::new(
        RestHandler::new("hold")
            .on(HttpMethod::Post, handle_hold)
            .on(HttpMethod::Delete, handle_unhold),
    );

    // /channels/{channelId}/play
    let play = Arc::new(
        RestHandler::new("play").on(HttpMethod::Post, handle_play),
    );

    // /channels/{channelId}/record
    let record = Arc::new(
        RestHandler::new("record").on(HttpMethod::Post, handle_record),
    );

    // /channels/{channelId}/variable
    let variable = Arc::new(
        RestHandler::new("variable")
            .on(HttpMethod::Get, handle_get_variable)
            .on(HttpMethod::Post, handle_set_variable),
    );

    // /channels/{channelId}/snoop
    let snoop = Arc::new(
        RestHandler::new("snoop").on(HttpMethod::Post, handle_snoop),
    );

    // /channels/{channelId}/dial
    let dial = Arc::new(
        RestHandler::new("dial").on(HttpMethod::Post, handle_dial),
    );

    // /channels/{channelId}/silence
    let silence = Arc::new(
        RestHandler::new("silence")
            .on(HttpMethod::Post, handle_start_silence)
            .on(HttpMethod::Delete, handle_stop_silence),
    );

    // /channels/{channelId}/move
    let move_handler = Arc::new(
        RestHandler::new("move").on(HttpMethod::Post, handle_move),
    );

    // Wire up the subtree
    let channel_by_id = Arc::new(
        RestHandler::new("{channelId}")
            .on(HttpMethod::Get, handle_get)
            .on(HttpMethod::Post, handle_originate_with_id)
            .on(HttpMethod::Delete, handle_hangup)
            .child(continue_handler)
            .child(redirect)
            .child(answer)
            .child(ring)
            .child(dtmf)
            .child(mute)
            .child(hold)
            .child(play)
            .child(record)
            .child(variable)
            .child(snoop)
            .child(dial)
            .child(silence)
            .child(move_handler),
    );

    

    Arc::new(
        RestHandler::new("channels")
            .on(HttpMethod::Get, handle_list)
            .on(HttpMethod::Post, handle_originate)
            .child(create)
            .child(external_media)
            .child(channel_by_id),
    )
}

/// Run an async future to completion from a synchronous route handler.
///
/// Mirrors the established pattern in `asterisk-core`'s
/// `pbx::substitute::try_call_function_sync`: on a multi-thread runtime
/// (the production HTTP listener) use `block_in_place`; on a
/// current-thread runtime (e.g. `#[tokio::test]`) drive the future from a
/// scratch thread so we neither panic nor deadlock; with no runtime at
/// all, build a temporary one.
fn run_async<F, T>(fut: F) -> T
where
    F: std::future::Future<Output = T> + Send,
    T: Send,
{
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => match handle.runtime_flavor() {
            tokio::runtime::RuntimeFlavor::MultiThread => {
                tokio::task::block_in_place(|| handle.block_on(fut))
            }
            _ => std::thread::scope(|s| {
                s.spawn(|| handle.block_on(fut))
                    .join()
                    .expect("run_async worker thread panicked")
            }),
        },
        Err(_) => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build fallback tokio runtime")
            .block_on(fut),
    }
}

// ---------------------------------------------------------------------------
// Handler implementations
// ---------------------------------------------------------------------------

/// GET /channels -- list all active channels.
fn handle_list(_req: &AriRequest, _server: &AriServer) -> AriResponse {
    // In a full implementation, this would query the channel registry.
    // For now, return an empty list.
    let channels: Vec<Channel> = Vec::new();
    AriResponse::ok(&channels)
}

/// POST /channels -- originate a new channel.
fn handle_originate(req: &AriRequest, server: &AriServer) -> AriResponse {
    let endpoint = match req.query_param("endpoint") {
        Some(ep) => ep.to_string(),
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing required parameter: endpoint".into(),
            ));
        }
    };

    let channel_id = req
        .query_param("channelId")
        .map(|s| s.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let channel = Channel {
        id: channel_id,
        name: format!("{}-{}", endpoint, uuid::Uuid::new_v4().as_simple()),
        state: "Down".to_string(),
        caller: AriCallerId {
            name: req.query_param("callerId").unwrap_or("").to_string(),
            number: String::new(),
        },
        connected: AriCallerId::default(),
        accountcode: String::new(),
        dialplan: DialplanCep {
            context: req.query_param("context").unwrap_or("default").to_string(),
            exten: req.query_param("extension").unwrap_or("s").to_string(),
            priority: req
                .query_param("priority")
                .and_then(|p| p.parse().ok())
                .unwrap_or(1),
            app_name: req.query_param("app").map(|s| s.to_string()),
            app_data: req.query_param("appArgs").map(|s| s.to_string()),
        },
        creationtime: chrono_now(),
        language: "en".to_string(),
        protocol_id: None,
    };

    // If a Stasis app was specified, register the channel with the app
    if let Some(app_name) = req.query_param("app") {
        if let Some(app) = server.app_registry.get_app(app_name) {
            app.add_channel(&channel.id);
        }
    }

    AriResponse::ok(&channel)
}

/// POST /channels/create -- create channel without dialing.
fn handle_create(req: &AriRequest, server: &AriServer) -> AriResponse {
    let endpoint = match req.query_param("endpoint") {
        Some(ep) => ep.to_string(),
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing required parameter: endpoint".into(),
            ));
        }
    };

    let app = match req.query_param("app") {
        Some(a) => a.to_string(),
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing required parameter: app".into(),
            ));
        }
    };

    let channel_id = req
        .query_param("channelId")
        .map(|s| s.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let channel = Channel {
        id: channel_id,
        name: format!("{}-{}", endpoint, uuid::Uuid::new_v4().as_simple()),
        state: "Down".to_string(),
        caller: AriCallerId::default(),
        connected: AriCallerId::default(),
        accountcode: String::new(),
        dialplan: DialplanCep {
            context: "default".to_string(),
            exten: "s".to_string(),
            priority: 1,
            app_name: Some(app.clone()),
            app_data: req.query_param("appArgs").map(|s| s.to_string()),
        },
        creationtime: chrono_now(),
        language: "en".to_string(),
        protocol_id: None,
    };

    if let Some(app_state) = server.app_registry.get_app(&app) {
        app_state.add_channel(&channel.id);
    }

    AriResponse::ok(&channel)
}

/// Build the ARI Channel model from a live core channel.
fn ari_channel_model(chan: &asterisk_core::channel::Channel) -> Channel {
    Channel {
        id: chan.unique_id.0.clone(),
        name: chan.name.clone(),
        state: chan.state.to_string(),
        caller: AriCallerId {
            name: chan.caller.id.name.name.clone(),
            number: chan.caller.id.number.number.clone(),
        },
        connected: AriCallerId::default(),
        accountcode: chan.accountcode.clone(),
        dialplan: DialplanCep {
            context: chan.context.clone(),
            exten: chan.exten.clone(),
            priority: chan.priority as i64,
            app_name: None,
            app_data: None,
        },
        creationtime: chrono_now(),
        language: chan.language.clone(),
        protocol_id: None,
    }
}

/// Formats accepted for externalMedia channels. Must stay in sync with the
/// UnicastRTP channel driver's payload-type table (asterisk-channels
/// `rtp_channel::supported_formats`); validated here as well so the route
/// can answer 400 without a registered driver.
const EXTERNAL_MEDIA_FORMATS: &[&str] = &["ulaw", "mulaw", "pcmu", "alaw", "pcma"];

/// POST /channels/externalMedia -- start an external media channel.
///
/// Creates a `UnicastRTP` channel bound to an external RTP endpoint
/// (`external_host` as `host:port`): audio written to the channel is sent
/// as RTP to the external endpoint, and RTP received back is readable from
/// the channel (bidirectional at the channel-driver level). The channel is
/// placed under the given Stasis application, mirroring
/// `ast_ari_channels_external_media` semantics.
fn handle_external_media(req: &AriRequest, server: &AriServer) -> AriResponse {
    // --- required parameters -------------------------------------------
    let app = match req.query_param("app") {
        Some(a) if !a.is_empty() => a.to_string(),
        _ => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing required parameter: app".into(),
            ));
        }
    };

    let external_host = match req.query_param("external_host") {
        Some(h) if !h.is_empty() => h.to_string(),
        _ => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing required parameter: external_host".into(),
            ));
        }
    };
    if external_host.parse::<std::net::SocketAddr>().is_err() {
        return AriResponse::error(&AriErrorKind::BadRequest(format!(
            "external_host '{}' is not a valid host:port address",
            external_host
        )));
    }

    let format = match req.query_param("format") {
        Some(f) if !f.is_empty() => f.to_string(),
        _ => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing required parameter: format".into(),
            ));
        }
    };
    if !EXTERNAL_MEDIA_FORMATS.contains(&format.as_str()) {
        return AriResponse::error(&AriErrorKind::BadRequest(format!(
            "format '{}' is not supported for external media (supported: {})",
            format,
            EXTERNAL_MEDIA_FORMATS.join(", ")
        )));
    }

    // --- optional parameters (only the RTP/UDP client mode exists) -----
    let encapsulation = req.query_param("encapsulation").unwrap_or("rtp");
    if encapsulation != "rtp" {
        return AriResponse::error(&AriErrorKind::BadRequest(format!(
            "encapsulation '{}' is not supported (supported: rtp)",
            encapsulation
        )));
    }
    let transport = req.query_param("transport").unwrap_or("udp");
    if transport != "udp" {
        return AriResponse::error(&AriErrorKind::BadRequest(format!(
            "transport '{}' is not supported (supported: udp)",
            transport
        )));
    }
    let connection_type = req.query_param("connection_type").unwrap_or("client");
    if connection_type != "client" {
        return AriResponse::error(&AriErrorKind::BadRequest(format!(
            "connection_type '{}' is not supported (supported: client)",
            connection_type
        )));
    }
    let direction = req.query_param("direction").unwrap_or("both");
    if direction != "both" {
        return AriResponse::error(&AriErrorKind::BadRequest(format!(
            "direction '{}' is not supported (supported: both)",
            direction
        )));
    }

    // --- channel id (client-supplied or generated) ----------------------
    let channel_id = match req.query_param("channelId") {
        Some(id) if !id.is_empty() => {
            if channel_store::find_by_uniqueid(id).is_some() {
                return AriResponse::error(&AriErrorKind::Conflict(format!(
                    "channel with id '{}' already exists",
                    id
                )));
            }
            id.to_string()
        }
        _ => channel_store::generate_uniqueid(),
    };

    // --- create the media channel via the UnicastRTP technology ---------
    let driver = match TECH_REGISTRY.find("UnicastRTP") {
        Some(d) => d,
        None => {
            return AriResponse::error(&AriErrorKind::Internal(
                "UnicastRTP channel technology is not registered".into(),
            ));
        }
    };

    let dest = format!("{}/{}", external_host, format);
    let mut chan = match run_async(driver.request(&dest, None)) {
        Ok(c) => c,
        Err(asterisk_types::AsteriskError::InvalidArgument(msg)) => {
            return AriResponse::error(&AriErrorKind::BadRequest(msg));
        }
        Err(e) => {
            return AriResponse::error(&AriErrorKind::Internal(format!(
                "failed to create external media channel: {}",
                e
            )));
        }
    };
    chan.unique_id = asterisk_core::channel::ChannelId(channel_id);

    // External media legs come up answered immediately (like chan_rtp).
    if let Err(e) = run_async(driver.call(&mut chan, &dest, 0)) {
        let _ = run_async(driver.hangup(&mut chan));
        return AriResponse::error(&AriErrorKind::Internal(format!(
            "failed to start external media channel: {}",
            e
        )));
    }

    // --- register in the global channel store (id preserved) ------------
    let chan_arc = match channel_store::try_register_channel(chan) {
        Ok(arc) => arc,
        Err(mut returned) => {
            // Lost a race on the channel id: release the driver entry and
            // its RTP socket before reporting the conflict (no leaks).
            let _ = run_async(driver.hangup(&mut returned));
            return AriResponse::error(&AriErrorKind::Conflict(format!(
                "channel with id '{}' already exists",
                returned.unique_id.0
            )));
        }
    };

    // --- enter the Stasis application ------------------------------------
    let model = {
        let guard = chan_arc.lock();
        ari_channel_model(&guard)
    };
    if let Some(app_state) = server.app_registry.get_app(&app) {
        app_state.add_channel(&model.id);
    }
    server.app_registry.dispatch_event(&AriEvent::StasisStart {
        base: EventBase {
            application: app.clone(),
            timestamp: chrono_now(),
            asterisk_id: None,
        },
        args: Vec::new(),
        channel: model.clone(),
        replace_channel: None,
    });

    AriResponse::ok(&model)
}

/// GET /channels/{channelId} -- get channel details.
fn handle_get(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _channel_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing channelId".into(),
            ));
        }
    };

    // In a full implementation, look up the channel.
    AriResponse::error(&AriErrorKind::NotFound("Channel not found".into()))
}

/// POST /channels/{channelId} -- originate with a specific ID.
fn handle_originate_with_id(req: &AriRequest, server: &AriServer) -> AriResponse {
    // Delegate to originate, the channelId comes from the path.
    handle_originate(req, server)
}

/// DELETE /channels/{channelId} -- hangup a channel.
///
/// For channels tracked in the global channel store (e.g. externalMedia
/// channels), this performs a real teardown: the technology driver's
/// `hangup()` releases the media plane (RTP socket + driver entry), the
/// channel is deregistered from the store, and StasisEnd/ChannelDestroyed
/// events are dispatched to subscribed applications. Channels not in the
/// store keep the historical no-op behavior.
fn handle_hangup(req: &AriRequest, server: &AriServer) -> AriResponse {
    let channel_id = match req.path_var(2) {
        Some(id) => id.to_string(),
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing channelId".into(),
            ));
        }
    };

    let _reason = req.query_param("reason").unwrap_or("normal");

    if let Some(chan_arc) = channel_store::find_by_uniqueid(&channel_id) {
        let (name, model) = {
            let guard = chan_arc.lock();
            (guard.name.clone(), ari_channel_model(&guard))
        };

        // Release the media plane via the technology driver (keyed by the
        // channel-name prefix, e.g. "UnicastRTP/..." -> "UnicastRTP"). The
        // driver drops its private entry and the bound RTP socket with it.
        let tech = name.split('/').next().unwrap_or("");
        if let Some(driver) = TECH_REGISTRY.find(tech) {
            let mut handle = asterisk_core::channel::Channel::new(&name);
            if let Err(e) = run_async(driver.hangup(&mut handle)) {
                tracing::debug!(channel = %name, error = %e, "driver hangup failed");
            }
        }

        // Mark hung up and drop from the global store.
        {
            let mut guard = chan_arc.lock();
            guard.hangup(asterisk_types::HangupCause::NormalClearing);
        }
        channel_store::deregister(&channel_id);

        // Leave Stasis: StasisEnd + ChannelDestroyed for subscribed apps.
        for app in server.app_registry.list_apps() {
            if !app.channel_ids.read().contains(&channel_id) {
                continue;
            }
            server.app_registry.dispatch_event(&AriEvent::StasisEnd {
                base: EventBase {
                    application: app.name.clone(),
                    timestamp: chrono_now(),
                    asterisk_id: None,
                },
                channel: model.clone(),
            });
            server.app_registry.dispatch_event(&AriEvent::ChannelDestroyed {
                base: EventBase {
                    application: app.name.clone(),
                    timestamp: chrono_now(),
                    asterisk_id: None,
                },
                cause: asterisk_types::HangupCause::NormalClearing as i32,
                cause_txt: "Normal Clearing".to_string(),
                channel: model.clone(),
            });
            app.remove_channel(&channel_id);
        }
    }

    AriResponse::no_content()
}

/// POST /channels/{channelId}/continue -- continue in the dialplan.
fn handle_continue(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _channel_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing channelId".into(),
            ));
        }
    };

    let _context = req.query_param("context");
    let _extension = req.query_param("extension");
    let _priority = req.query_param("priority");
    let _label = req.query_param("label");

    AriResponse::no_content()
}

/// POST /channels/{channelId}/redirect -- redirect channel to a different endpoint.
fn handle_redirect(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _channel_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing channelId".into(),
            ));
        }
    };

    let _endpoint = match req.query_param("endpoint") {
        Some(ep) => ep,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing required parameter: endpoint".into(),
            ));
        }
    };

    AriResponse::no_content()
}

/// POST /channels/{channelId}/answer -- answer the channel.
fn handle_answer(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _channel_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing channelId".into(),
            ));
        }
    };

    AriResponse::no_content()
}

/// POST /channels/{channelId}/ring -- start ringing.
fn handle_start_ring(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _channel_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing channelId".into(),
            ));
        }
    };

    AriResponse::no_content()
}

/// DELETE /channels/{channelId}/ring -- stop ringing.
fn handle_stop_ring(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _channel_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing channelId".into(),
            ));
        }
    };

    AriResponse::no_content()
}

/// POST /channels/{channelId}/dtmf -- send DTMF digits.
fn handle_send_dtmf(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _channel_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing channelId".into(),
            ));
        }
    };

    let _dtmf = match req.query_param("dtmf") {
        Some(d) => d,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing required parameter: dtmf".into(),
            ));
        }
    };

    let _before = req.query_param("before").and_then(|v| v.parse::<i32>().ok());
    let _between = req.query_param("between").and_then(|v| v.parse::<i32>().ok());
    let _duration = req.query_param("duration").and_then(|v| v.parse::<i32>().ok());
    let _after = req.query_param("after").and_then(|v| v.parse::<i32>().ok());

    AriResponse::no_content()
}

/// POST /channels/{channelId}/mute -- mute the channel.
fn handle_mute(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _channel_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing channelId".into(),
            ));
        }
    };

    let _direction = req.query_param("direction").unwrap_or("both");

    AriResponse::no_content()
}

/// DELETE /channels/{channelId}/mute -- unmute the channel.
fn handle_unmute(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _channel_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing channelId".into(),
            ));
        }
    };

    let _direction = req.query_param("direction").unwrap_or("both");

    AriResponse::no_content()
}

/// POST /channels/{channelId}/hold -- put channel on hold.
fn handle_hold(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _channel_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing channelId".into(),
            ));
        }
    };

    AriResponse::no_content()
}

/// DELETE /channels/{channelId}/hold -- remove channel from hold.
fn handle_unhold(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _channel_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing channelId".into(),
            ));
        }
    };

    AriResponse::no_content()
}

/// POST /channels/{channelId}/play -- start playback of media.
fn handle_play(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _channel_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing channelId".into(),
            ));
        }
    };

    let media_strs = req.query_params_multi("media");
    if media_strs.is_empty() {
        return AriResponse::error(&AriErrorKind::BadRequest(
            "missing required parameter: media".into(),
        ));
    }

    let playback_id = req
        .query_param("playbackId")
        .map(|s| s.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let playback = Playback {
        id: playback_id,
        media_uri: media_strs.first().unwrap_or(&"").to_string(),
        next_media_uri: media_strs.get(1).map(|s| s.to_string()),
        target_uri: format!("channel:{}", _channel_id),
        language: req.query_param("lang").map(|s| s.to_string()),
        state: PlaybackState::Queued,
    };

    AriResponse::ok(&playback)
}

/// POST /channels/{channelId}/record -- start recording.
fn handle_record(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _channel_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing channelId".into(),
            ));
        }
    };

    let name = match req.query_param("name") {
        Some(n) => n.to_string(),
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing required parameter: name".into(),
            ));
        }
    };

    let format = match req.query_param("format") {
        Some(f) => f.to_string(),
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing required parameter: format".into(),
            ));
        }
    };

    let recording = LiveRecording {
        name,
        format,
        target_uri: Some(format!("channel:{}", _channel_id)),
        state: RecordingState::Recording,
        duration: Some(0),
        silence_duration: None,
        talking_duration: None,
        cause: None,
    };

    AriResponse::ok(&recording)
}

/// GET /channels/{channelId}/variable -- get a channel variable.
///
/// Channels tracked in the global store return the live variable value
/// (e.g. `UNICASTRTP_LOCAL_PORT` on externalMedia channels); unknown
/// channels/variables keep the historical empty-value behavior.
fn handle_get_variable(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let channel_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing channelId".into(),
            ));
        }
    };

    let variable_name = match req.query_param("variable") {
        Some(v) => v,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing required parameter: variable".into(),
            ));
        }
    };

    let value = channel_store::find_by_uniqueid(channel_id)
        .and_then(|chan| chan.lock().variables.get(variable_name).cloned())
        .unwrap_or_default();

    let variable = Variable { value };

    AriResponse::ok(&variable)
}

/// POST /channels/{channelId}/variable -- set a channel variable.
fn handle_set_variable(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _channel_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing channelId".into(),
            ));
        }
    };

    let _variable_name = match req.query_param("variable") {
        Some(v) => v,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing required parameter: variable".into(),
            ));
        }
    };

    let _value = req.query_param("value").unwrap_or("");

    AriResponse::no_content()
}

/// POST /channels/{channelId}/snoop -- create a snoop channel.
fn handle_snoop(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _channel_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing channelId".into(),
            ));
        }
    };

    let _app = match req.query_param("app") {
        Some(a) => a,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing required parameter: app".into(),
            ));
        }
    };

    let _spy = req.query_param("spy").unwrap_or("none");
    let _whisper = req.query_param("whisper").unwrap_or("none");

    let snoop_id = req
        .query_param("snoopId")
        .map(|s| s.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let snoop_channel = Channel {
        id: snoop_id,
        name: format!("Snoop/{}-{}", _channel_id, uuid::Uuid::new_v4().as_simple()),
        state: "Up".to_string(),
        caller: AriCallerId::default(),
        connected: AriCallerId::default(),
        accountcode: String::new(),
        dialplan: DialplanCep::default(),
        creationtime: chrono_now(),
        language: "en".to_string(),
        protocol_id: None,
    };

    AriResponse::ok(&snoop_channel)
}

/// POST /channels/{channelId}/dial -- dial a created channel.
fn handle_dial(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _channel_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing channelId".into(),
            ));
        }
    };

    let _caller = req.query_param("caller");
    let _timeout = req
        .query_param("timeout")
        .and_then(|v| v.parse::<i32>().ok())
        .unwrap_or(30);

    AriResponse::no_content()
}

/// POST /channels/{channelId}/silence -- start silence generator.
fn handle_start_silence(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _channel_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing channelId".into(),
            ));
        }
    };

    AriResponse::no_content()
}

/// DELETE /channels/{channelId}/silence -- stop silence generator.
fn handle_stop_silence(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _channel_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing channelId".into(),
            ));
        }
    };

    AriResponse::no_content()
}

/// POST /channels/{channelId}/move -- move channel to another Stasis app.
fn handle_move(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _channel_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing channelId".into(),
            ));
        }
    };

    let _app = match req.query_param("app") {
        Some(a) => a,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing required parameter: app".into(),
            ));
        }
    };

    AriResponse::no_content()
}

/// Get a simple ISO-8601 timestamp string.
pub fn chrono_now() -> String {
    // Using a simple format without pulling in the chrono crate
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}Z", now.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::{AriConfig, AriServer};

    /// Build an AriRequest for handler-level tests.
    fn ari_request(method: HttpMethod, path: &str, params: &[(&str, &str)]) -> AriRequest {
        let path_segments: Vec<String> = path
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
        let mut query_params: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for (k, v) in params {
            query_params
                .entry(k.to_string())
                .or_default()
                .push(v.to_string());
        }
        AriRequest {
            method,
            path: path.to_string(),
            path_segments,
            query_params,
            body: None,
            username: None,
        }
    }

    fn external_media_request(params: &[(&str, &str)]) -> AriRequest {
        ari_request(HttpMethod::Post, "/ari/channels/externalMedia", params)
    }

    fn body_string(resp: &AriResponse) -> String {
        String::from_utf8_lossy(resp.body.as_deref().unwrap_or_default()).to_string()
    }

    #[test]
    fn external_media_requires_app() {
        let server = AriServer::new(AriConfig::default());
        let req = external_media_request(&[
            ("external_host", "127.0.0.1:9999"),
            ("format", "ulaw"),
        ]);
        let resp = handle_external_media(&req, &server);
        assert_eq!(resp.status, 400);
        assert!(body_string(&resp).contains("app"), "{}", body_string(&resp));
    }

    #[test]
    fn external_media_requires_external_host() {
        let server = AriServer::new(AriConfig::default());
        let req = external_media_request(&[("app", "myapp"), ("format", "ulaw")]);
        let resp = handle_external_media(&req, &server);
        assert_eq!(resp.status, 400);
        assert!(body_string(&resp).contains("external_host"));
    }

    #[test]
    fn external_media_requires_format() {
        let server = AriServer::new(AriConfig::default());
        let req = external_media_request(&[
            ("app", "myapp"),
            ("external_host", "127.0.0.1:9999"),
        ]);
        let resp = handle_external_media(&req, &server);
        assert_eq!(resp.status, 400);
        assert!(body_string(&resp).contains("format"));
    }

    #[test]
    fn external_media_rejects_invalid_host() {
        let server = AriServer::new(AriConfig::default());
        for bad in ["not-a-host", "127.0.0.1", "127.0.0.1:notaport", ":5000"] {
            let req = external_media_request(&[
                ("app", "myapp"),
                ("external_host", bad),
                ("format", "ulaw"),
            ]);
            let resp = handle_external_media(&req, &server);
            assert_eq!(resp.status, 400, "external_host '{}' must be rejected", bad);
            assert!(body_string(&resp).contains("external_host"));
        }
    }

    #[test]
    fn external_media_rejects_unsupported_format() {
        let server = AriServer::new(AriConfig::default());
        let req = external_media_request(&[
            ("app", "myapp"),
            ("external_host", "127.0.0.1:9999"),
            ("format", "g729"),
        ]);
        let resp = handle_external_media(&req, &server);
        assert_eq!(resp.status, 400);
        assert!(body_string(&resp).contains("format 'g729'"));
    }

    #[test]
    fn external_media_rejects_unsupported_optionals() {
        let server = AriServer::new(AriConfig::default());
        let base = [
            ("app", "myapp"),
            ("external_host", "127.0.0.1:9999"),
            ("format", "ulaw"),
        ];
        for (key, value) in [
            ("encapsulation", "audiosocket"),
            ("transport", "tcp"),
            ("connection_type", "server"),
            ("direction", "out"),
        ] {
            let mut params = base.to_vec();
            params.push((key, value));
            let resp = handle_external_media(&external_media_request(&params), &server);
            assert_eq!(resp.status, 400, "{}={} must be rejected", key, value);
            assert!(
                body_string(&resp).contains(value),
                "error should mention the offending value: {}",
                body_string(&resp)
            );
        }
    }

    /// Valid parameters but no UnicastRTP technology registered in this
    /// process: the route must fail cleanly with a 500, not panic.
    /// (No test in this crate registers channel technologies.)
    #[test]
    fn external_media_without_driver_is_internal_error() {
        let server = AriServer::new(AriConfig::default());
        let req = external_media_request(&[
            ("app", "myapp"),
            ("external_host", "127.0.0.1:9999"),
            ("format", "ulaw"),
        ]);
        let resp = handle_external_media(&req, &server);
        assert_eq!(resp.status, 500);
        assert!(body_string(&resp).contains("UnicastRTP"));
    }

    /// The externalMedia segment must resolve to its own handler, not fall
    /// through to the `{channelId}` wildcard (POST originate-with-id): the
    /// error for a bare request must complain about `app`, not `endpoint`.
    #[test]
    fn external_media_route_wins_over_wildcard() {
        let mut server = AriServer::new(AriConfig::default());
        server.install_routes();
        let req = external_media_request(&[]);
        let resp = server.handle_request(&req);
        assert_eq!(resp.status, 400);
        let body = body_string(&resp);
        assert!(
            body.contains("app") && !body.contains("endpoint"),
            "externalMedia must not be routed to the channelId wildcard: {}",
            body
        );
    }
}
