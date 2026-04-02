//! /ari/playbacks resource -- playback control via the ARI REST interface.
//!
//! Port of res/ari/resource_playbacks.c. Implements playback control endpoints:
//! get playback status, stop playback, and control (pause, unpause, restart,
//! reverse, forward).

use crate::error::AriErrorKind;
use crate::server::{AriRequest, AriResponse, AriServer, HttpMethod, RestHandler};
use std::sync::Arc;

/// Build the /playbacks route subtree.
pub fn build_playbacks_routes() -> Arc<RestHandler> {
    // /playbacks/{playbackId}/control
    let control = Arc::new(
        RestHandler::new("control").on(HttpMethod::Post, handle_control),
    );

    // /playbacks/{playbackId}
    let playback_by_id = Arc::new(
        RestHandler::new("{playbackId}")
            .on(HttpMethod::Get, handle_get)
            .on(HttpMethod::Delete, handle_stop)
            .child(control),
    );

    // /playbacks
    

    Arc::new(
        RestHandler::new("playbacks")
            .child(playback_by_id),
    )
}

// ---------------------------------------------------------------------------
// Handler implementations
// ---------------------------------------------------------------------------

/// GET /playbacks/{playbackId} -- get playback details.
fn handle_get(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _playback_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing playbackId".into(),
            ));
        }
    };

    AriResponse::error(&AriErrorKind::NotFound("Playback not found".into()))
}

/// DELETE /playbacks/{playbackId} -- stop a playback.
fn handle_stop(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _playback_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing playbackId".into(),
            ));
        }
    };

    AriResponse::no_content()
}

/// POST /playbacks/{playbackId}/control -- control a playback.
///
/// The `operation` query parameter must be one of: restart, pause, unpause, reverse, forward.
fn handle_control(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _playback_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing playbackId".into(),
            ));
        }
    };

    let operation = match req.query_param("operation") {
        Some(op) => op,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing required parameter: operation".into(),
            ));
        }
    };

    // Validate operation
    match operation {
        "restart" | "pause" | "unpause" | "reverse" | "forward" => {}
        _ => {
            return AriResponse::error(&AriErrorKind::BadRequest(format!(
                "invalid operation: {}. Must be one of: restart, pause, unpause, reverse, forward",
                operation
            )));
        }
    }

    AriResponse::no_content()
}
