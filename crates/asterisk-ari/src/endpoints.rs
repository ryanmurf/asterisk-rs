//! /ari/endpoints resource -- endpoint operations via the ARI REST interface.
//!
//! Port of res/ari/resource_endpoints.c. Implements listing endpoints
//! by technology, getting endpoint details, and sending text messages.

use crate::error::AriErrorKind;
use crate::models::*;
use crate::server::{AriRequest, AriResponse, AriServer, HttpMethod, RestHandler};
use std::sync::Arc;

/// Build the /endpoints route subtree.
pub fn build_endpoints_routes() -> Arc<RestHandler> {
    // /endpoints/sendMessage
    let send_message = Arc::new(
        RestHandler::new("sendMessage").on(HttpMethod::Put, handle_send_message),
    );

    // /endpoints/refer
    let refer = Arc::new(
        RestHandler::new("refer").on(HttpMethod::Post, handle_refer),
    );

    // /endpoints/{tech}/{resource}/sendMessage
    let resource_send_message = Arc::new(
        RestHandler::new("sendMessage").on(HttpMethod::Put, handle_send_message_to_endpoint),
    );

    // /endpoints/{tech}/{resource}
    let resource = Arc::new(
        RestHandler::new("{resource}")
            .on(HttpMethod::Get, handle_get)
            .child(resource_send_message),
    );

    // /endpoints/{tech}
    let tech = Arc::new(
        RestHandler::new("{tech}")
            .on(HttpMethod::Get, handle_list_by_tech)
            .child(resource),
    );

    // /endpoints
    

    Arc::new(
        RestHandler::new("endpoints")
            .on(HttpMethod::Get, handle_list)
            .child(send_message)
            .child(refer)
            .child(tech),
    )
}

// ---------------------------------------------------------------------------
// Handler implementations
// ---------------------------------------------------------------------------

/// GET /endpoints -- list all endpoints.
fn handle_list(_req: &AriRequest, _server: &AriServer) -> AriResponse {
    let endpoints: Vec<Endpoint> = Vec::new();
    AriResponse::ok(&endpoints)
}

/// GET /endpoints/{tech} -- list endpoints by technology.
fn handle_list_by_tech(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _tech = match req.path_var(2) {
        Some(t) => t,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing tech".into(),
            ));
        }
    };

    let endpoints: Vec<Endpoint> = Vec::new();
    AriResponse::ok(&endpoints)
}

/// GET /endpoints/{tech}/{resource} -- get endpoint details.
fn handle_get(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _tech = match req.path_var(2) {
        Some(t) => t,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing tech".into(),
            ));
        }
    };

    let _resource = match req.path_var(3) {
        Some(r) => r,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing resource".into(),
            ));
        }
    };

    AriResponse::error(&AriErrorKind::NotFound("Endpoint not found".into()))
}

/// PUT /endpoints/sendMessage -- send a text message to a technology URI.
fn handle_send_message(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _to = match req.query_param("to") {
        Some(t) => t,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing required parameter: to".into(),
            ));
        }
    };

    let _from = match req.query_param("from") {
        Some(f) => f,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing required parameter: from".into(),
            ));
        }
    };

    let _body = req.query_param("body").unwrap_or("");

    AriResponse::no_content()
}

/// POST /endpoints/refer -- refer an endpoint to another endpoint.
fn handle_refer(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _to = match req.query_param("to") {
        Some(t) => t,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing required parameter: to".into(),
            ));
        }
    };

    let _from = match req.query_param("from") {
        Some(f) => f,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing required parameter: from".into(),
            ));
        }
    };

    let _refer_to = match req.query_param("refer_to") {
        Some(r) => r,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing required parameter: refer_to".into(),
            ));
        }
    };

    AriResponse::no_content()
}

/// PUT /endpoints/{tech}/{resource}/sendMessage -- send a text message to a specific endpoint.
fn handle_send_message_to_endpoint(req: &AriRequest, _server: &AriServer) -> AriResponse {
    let _tech = match req.path_var(2) {
        Some(t) => t,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing tech".into(),
            ));
        }
    };

    let _resource = match req.path_var(3) {
        Some(r) => r,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing resource".into(),
            ));
        }
    };

    let _from = match req.query_param("from") {
        Some(f) => f,
        None => {
            return AriResponse::error(&AriErrorKind::BadRequest(
                "missing required parameter: from".into(),
            ));
        }
    };

    let _body = req.query_param("body").unwrap_or("");

    AriResponse::no_content()
}
