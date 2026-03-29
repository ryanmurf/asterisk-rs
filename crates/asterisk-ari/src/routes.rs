//! Route tree builder -- assembles all ARI resource routes into a single tree.
//!
//! Called by AriServer::install_routes() to wire up the complete REST handler
//! hierarchy under /ari.

use crate::applications::build_applications_routes;
use crate::asterisk_resource::build_asterisk_routes;
use crate::bridges::build_bridges_routes;
use crate::channels::build_channels_routes;
use crate::device_states::build_device_states_routes;
use crate::endpoints::build_endpoints_routes;
use crate::mailboxes::build_mailboxes_routes;
use crate::playbacks::build_playbacks_routes;
use crate::recordings::build_recordings_routes;
use crate::server::RestHandler;
use crate::sounds::build_sounds_routes;
use std::sync::Arc;

/// Build the complete ARI route tree rooted at "ari".
pub fn build_route_tree() -> Arc<RestHandler> {
    let root = Arc::new(
        RestHandler::new("ari")
            .child(build_channels_routes())
            .child(build_bridges_routes())
            .child(build_endpoints_routes())
            .child(build_recordings_routes())
            .child(build_playbacks_routes())
            .child(build_sounds_routes())
            .child(build_applications_routes())
            .child(build_asterisk_routes())
            .child(build_device_states_routes())
            .child(build_mailboxes_routes()),
    );

    root
}
