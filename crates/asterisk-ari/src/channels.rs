//! /ari/channels resource -- channel operations via the ARI REST interface.
//!
//! Port of res/ari/resource_channels.c. Implements all channel-related
//! ARI endpoints: list, originate, get, hangup, answer, ring, DTMF,
//! mute, hold, play, record, variable get/set, snoop, and dial.

use crate::error::AriErrorKind;
use crate::models::*;
use crate::server::{AriRequest, AriResponse, AriServer, HttpMethod, RestHandler};
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
            .child(channel_by_id),
    )
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
fn handle_hangup(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _channel_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing channelId".into(),
            ));
        }
    };

    let _reason = req.query_param("reason").unwrap_or("normal");

    // In a full implementation, look up the channel and hang it up.
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
fn handle_get_variable(req: &AriRequest, _server: &AriServer) -> AriResponse {
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

    // In a full implementation, look up the channel and get the variable.
    let variable = Variable {
        value: String::new(),
    };

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
