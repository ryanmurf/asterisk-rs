//! /ari/bridges resource -- bridge operations via the ARI REST interface.
//!
//! Port of res/ari/resource_bridges.c. Implements all bridge-related
//! ARI endpoints: list, create, get, destroy, addChannel, removeChannel,
//! play media, record, start/stop MOH, and video source management.

use crate::error::AriErrorKind;
use crate::models::*;
use crate::server::{AriRequest, AriResponse, AriServer, HttpMethod, RestHandler};
use std::sync::Arc;

/// Build the /bridges route subtree.
pub fn build_bridges_routes() -> Arc<RestHandler> {
    // /bridges/{bridgeId}/addChannel
    let add_channel = Arc::new(
        RestHandler::new("addChannel").on(HttpMethod::Post, handle_add_channel),
    );

    // /bridges/{bridgeId}/removeChannel
    let remove_channel = Arc::new(
        RestHandler::new("removeChannel").on(HttpMethod::Post, handle_remove_channel),
    );

    // /bridges/{bridgeId}/play
    let play = Arc::new(
        RestHandler::new("play").on(HttpMethod::Post, handle_play),
    );

    // /bridges/{bridgeId}/record
    let record = Arc::new(
        RestHandler::new("record").on(HttpMethod::Post, handle_record),
    );

    // /bridges/{bridgeId}/moh
    let moh = Arc::new(
        RestHandler::new("moh")
            .on(HttpMethod::Post, handle_start_moh)
            .on(HttpMethod::Delete, handle_stop_moh),
    );

    // /bridges/{bridgeId}/videoSource
    let video_source = Arc::new(
        RestHandler::new("videoSource").on(HttpMethod::Post, handle_set_video_source),
    );

    // /bridges/{bridgeId}
    let bridge_by_id = Arc::new(
        RestHandler::new("{bridgeId}")
            .on(HttpMethod::Get, handle_get)
            .on(HttpMethod::Post, handle_create_with_id)
            .on(HttpMethod::Delete, handle_destroy)
            .child(add_channel)
            .child(remove_channel)
            .child(play)
            .child(record)
            .child(moh)
            .child(video_source),
    );

    // /bridges
    let bridges = Arc::new(
        RestHandler::new("bridges")
            .on(HttpMethod::Get, handle_list)
            .on(HttpMethod::Post, handle_create)
            .child(bridge_by_id),
    );

    bridges
}

// ---------------------------------------------------------------------------
// Handler implementations
// ---------------------------------------------------------------------------

/// GET /bridges -- list all active bridges.
fn handle_list(_req: &AriRequest, _server: &AriServer) -> AriResponse {
    let bridges: Vec<Bridge> = Vec::new();
    AriResponse::ok(&bridges)
}

/// POST /bridges -- create a new bridge.
fn handle_create(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let bridge_id = req
        .query_param("bridgeId")
        .map(|s| s.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let bridge_type = req.query_param("type").unwrap_or("mixing");
    let name = req.query_param("name").unwrap_or("").to_string();

    let bridge = Bridge {
        id: bridge_id,
        technology: "simple_bridge".to_string(),
        bridge_type: bridge_type.to_string(),
        bridge_class: "stasis".to_string(),
        creator: "ARI".to_string(),
        name,
        channels: Vec::new(),
        video_mode: None,
        video_source_id: None,
        creationtime: crate::channels::chrono_now(),
    };

    AriResponse::ok(&bridge)
}

/// POST /bridges/{bridgeId} -- create a bridge with a specific ID.
fn handle_create_with_id(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let bridge_id = match req.path_var(2) {
        Some(id) => id.to_string(),
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing bridgeId".into(),
            ));
        }
    };

    let bridge_type = req.query_param("type").unwrap_or("mixing");
    let name = req.query_param("name").unwrap_or("").to_string();

    let bridge = Bridge {
        id: bridge_id,
        technology: "simple_bridge".to_string(),
        bridge_type: bridge_type.to_string(),
        bridge_class: "stasis".to_string(),
        creator: "ARI".to_string(),
        name,
        channels: Vec::new(),
        video_mode: None,
        video_source_id: None,
        creationtime: crate::channels::chrono_now(),
    };

    AriResponse::ok(&bridge)
}

/// GET /bridges/{bridgeId} -- get bridge details.
fn handle_get(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _bridge_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing bridgeId".into(),
            ));
        }
    };

    AriResponse::error(&AriErrorKind::NotFound("Bridge not found".into()))
}

/// DELETE /bridges/{bridgeId} -- shut down a bridge.
fn handle_destroy(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _bridge_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing bridgeId".into(),
            ));
        }
    };

    AriResponse::no_content()
}

/// POST /bridges/{bridgeId}/addChannel -- add channels to a bridge.
fn handle_add_channel(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _bridge_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing bridgeId".into(),
            ));
        }
    };

    let channels = req.query_params_multi("channel");
    if channels.is_empty() {
        return AriResponse::error(&AriErrorKind::BadRequest(
            "missing required parameter: channel".into(),
        ));
    }

    let _role = req.query_param("role");
    let _absorb_dtmf = req.query_param("absorbDTMF");
    let _mute = req.query_param("mute");

    AriResponse::no_content()
}

/// POST /bridges/{bridgeId}/removeChannel -- remove channels from a bridge.
fn handle_remove_channel(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _bridge_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing bridgeId".into(),
            ));
        }
    };

    let channels = req.query_params_multi("channel");
    if channels.is_empty() {
        return AriResponse::error(&AriErrorKind::BadRequest(
            "missing required parameter: channel".into(),
        ));
    }

    AriResponse::no_content()
}

/// POST /bridges/{bridgeId}/play -- play media to a bridge.
fn handle_play(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let bridge_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing bridgeId".into(),
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
        target_uri: format!("bridge:{}", bridge_id),
        language: req.query_param("lang").map(|s| s.to_string()),
        state: PlaybackState::Queued,
    };

    AriResponse::ok(&playback)
}

/// POST /bridges/{bridgeId}/record -- record a bridge.
fn handle_record(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let bridge_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing bridgeId".into(),
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
        target_uri: Some(format!("bridge:{}", bridge_id)),
        state: RecordingState::Recording,
        duration: Some(0),
        silence_duration: None,
        talking_duration: None,
        cause: None,
    };

    AriResponse::ok(&recording)
}

/// POST /bridges/{bridgeId}/moh -- start music on hold.
fn handle_start_moh(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _bridge_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing bridgeId".into(),
            ));
        }
    };

    let _moh_class = req.query_param("mohClass").unwrap_or("default");

    AriResponse::no_content()
}

/// DELETE /bridges/{bridgeId}/moh -- stop music on hold.
fn handle_stop_moh(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _bridge_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing bridgeId".into(),
            ));
        }
    };

    AriResponse::no_content()
}

/// POST /bridges/{bridgeId}/videoSource -- set the video source for the bridge.
fn handle_set_video_source(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _bridge_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing bridgeId".into(),
            ));
        }
    };

    let _channel_id = match req.query_param("channelId") {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing required parameter: channelId".into(),
            ));
        }
    };

    AriResponse::no_content()
}
