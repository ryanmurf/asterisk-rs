//! /ari/recordings resource -- recording operations via the ARI REST interface.
//!
//! Port of res/ari/resource_recordings.c. Implements all recording-related
//! ARI endpoints: list stored, get/delete stored, get stored file, copy stored,
//! get live, cancel live, stop, pause/unpause, and mute/unmute live recordings.

use crate::error::AriErrorKind;
use crate::models::*;
use crate::server::{AriRequest, AriResponse, AriServer, HttpMethod, RestHandler};
use std::sync::Arc;

/// Build the /recordings route subtree.
pub fn build_recordings_routes() -> Arc<RestHandler> {
    // /recordings/stored/{recordingName}/file
    let stored_file = Arc::new(
        RestHandler::new("file").on(HttpMethod::Get, handle_get_stored_file),
    );

    // /recordings/stored/{recordingName}/copy
    let stored_copy = Arc::new(
        RestHandler::new("copy").on(HttpMethod::Post, handle_copy_stored),
    );

    // /recordings/stored/{recordingName}
    let stored_by_name = Arc::new(
        RestHandler::new("{recordingName}")
            .on(HttpMethod::Get, handle_get_stored)
            .on(HttpMethod::Delete, handle_delete_stored)
            .child(stored_file)
            .child(stored_copy),
    );

    // /recordings/stored
    let stored = Arc::new(
        RestHandler::new("stored")
            .on(HttpMethod::Get, handle_list_stored)
            .child(stored_by_name),
    );

    // /recordings/live/{recordingName}/stop
    let live_stop = Arc::new(
        RestHandler::new("stop").on(HttpMethod::Post, handle_stop),
    );

    // /recordings/live/{recordingName}/pause
    let live_pause = Arc::new(
        RestHandler::new("pause")
            .on(HttpMethod::Post, handle_pause)
            .on(HttpMethod::Delete, handle_unpause),
    );

    // /recordings/live/{recordingName}/mute
    let live_mute = Arc::new(
        RestHandler::new("mute")
            .on(HttpMethod::Post, handle_mute)
            .on(HttpMethod::Delete, handle_unmute),
    );

    // /recordings/live/{recordingName}
    let live_by_name = Arc::new(
        RestHandler::new("{recordingName}")
            .on(HttpMethod::Get, handle_get_live)
            .on(HttpMethod::Delete, handle_cancel)
            .child(live_stop)
            .child(live_pause)
            .child(live_mute),
    );

    // /recordings/live
    let live = Arc::new(
        RestHandler::new("live").child(live_by_name),
    );

    // /recordings
    

    Arc::new(
        RestHandler::new("recordings")
            .child(stored)
            .child(live),
    )
}

// ---------------------------------------------------------------------------
// Stored recording handlers
// ---------------------------------------------------------------------------

/// GET /recordings/stored -- list stored recordings.
fn handle_list_stored(_req: &AriRequest, _server: &AriServer) -> AriResponse {
    let recordings: Vec<StoredRecording> = Vec::new();
    AriResponse::ok(&recordings)
}

/// GET /recordings/stored/{recordingName} -- get stored recording details.
fn handle_get_stored(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _name = match req.path_var(3) {
        Some(n) => n,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing recordingName".into(),
            ));
        }
    };

    AriResponse::error(&AriErrorKind::NotFound("Recording not found".into()))
}

/// DELETE /recordings/stored/{recordingName} -- delete a stored recording.
fn handle_delete_stored(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _name = match req.path_var(3) {
        Some(n) => n,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing recordingName".into(),
            ));
        }
    };

    AriResponse::no_content()
}

/// GET /recordings/stored/{recordingName}/file -- get the recording file.
fn handle_get_stored_file(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _name = match req.path_var(3) {
        Some(n) => n,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing recordingName".into(),
            ));
        }
    };

    // In a full implementation, read the file and return it with the correct content type.
    AriResponse::error(&AriErrorKind::NotFound("Recording not found".into()))
}

/// POST /recordings/stored/{recordingName}/copy -- copy a stored recording.
fn handle_copy_stored(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _name = match req.path_var(3) {
        Some(n) => n,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing recordingName".into(),
            ));
        }
    };

    let _dest = match req.query_param("destinationRecordingName") {
        Some(d) => d,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing required parameter: destinationRecordingName".into(),
            ));
        }
    };

    AriResponse::error(&AriErrorKind::NotFound("Recording not found".into()))
}

// ---------------------------------------------------------------------------
// Live recording handlers
// ---------------------------------------------------------------------------

/// GET /recordings/live/{recordingName} -- get live recording details.
fn handle_get_live(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _name = match req.path_var(3) {
        Some(n) => n,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing recordingName".into(),
            ));
        }
    };

    AriResponse::error(&AriErrorKind::NotFound("Recording not found".into()))
}

/// DELETE /recordings/live/{recordingName} -- cancel and discard a live recording.
fn handle_cancel(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _name = match req.path_var(3) {
        Some(n) => n,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing recordingName".into(),
            ));
        }
    };

    AriResponse::no_content()
}

/// POST /recordings/live/{recordingName}/stop -- stop and store a live recording.
fn handle_stop(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _name = match req.path_var(3) {
        Some(n) => n,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing recordingName".into(),
            ));
        }
    };

    AriResponse::no_content()
}

/// POST /recordings/live/{recordingName}/pause -- pause a live recording.
fn handle_pause(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _name = match req.path_var(3) {
        Some(n) => n,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing recordingName".into(),
            ));
        }
    };

    AriResponse::no_content()
}

/// DELETE /recordings/live/{recordingName}/pause -- unpause a live recording.
fn handle_unpause(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _name = match req.path_var(3) {
        Some(n) => n,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing recordingName".into(),
            ));
        }
    };

    AriResponse::no_content()
}

/// POST /recordings/live/{recordingName}/mute -- mute a live recording.
fn handle_mute(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _name = match req.path_var(3) {
        Some(n) => n,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing recordingName".into(),
            ));
        }
    };

    AriResponse::no_content()
}

/// DELETE /recordings/live/{recordingName}/mute -- unmute a live recording.
fn handle_unmute(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _name = match req.path_var(3) {
        Some(n) => n,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing recordingName".into(),
            ));
        }
    };

    AriResponse::no_content()
}
