//! /ari/sounds resource -- sound listing and lookup via the ARI REST interface.
//!
//! Port of res/ari/resource_sounds.c. Implements endpoints for listing
//! available sound files and retrieving details about specific sounds.

use crate::error::AriErrorKind;
use crate::models::*;
use crate::server::{AriRequest, AriResponse, AriServer, HttpMethod, RestHandler};
use std::sync::Arc;

/// Build the /sounds route subtree.
pub fn build_sounds_routes() -> Arc<RestHandler> {
    // /sounds/{soundId}
    let sound_by_id = Arc::new(
        RestHandler::new("{soundId}").on(HttpMethod::Get, handle_get),
    );

    // /sounds
    let sounds = Arc::new(
        RestHandler::new("sounds")
            .on(HttpMethod::Get, handle_list)
            .child(sound_by_id),
    );

    sounds
}

// ---------------------------------------------------------------------------
// Handler implementations
// ---------------------------------------------------------------------------

/// GET /sounds -- list all available sounds.
///
/// Optional query parameters:
/// - `lang` - filter by language
/// - `format` - filter by format
fn handle_list(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _lang = req.query_param("lang");
    let _format = req.query_param("format");

    // In a full implementation, scan the sounds directory and filter.
    let sounds: Vec<Sound> = Vec::new();
    AriResponse::ok(&sounds)
}

/// GET /sounds/{soundId} -- get details of a specific sound.
fn handle_get(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _sound_id = match req.path_var(2) {
        Some(id) => id,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing soundId".into(),
            ));
        }
    };

    // In a full implementation, look up the sound file.
    AriResponse::error(&AriErrorKind::NotFound("Sound not found".into()))
}
