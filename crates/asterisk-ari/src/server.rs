//! ARI HTTP server -- mounts routes, handles authentication, manages WebSocket sessions.
//!
//! This is the Rust equivalent of res_ari.c. It provides the main entry point
//! for the ARI HTTP server, routing requests to the appropriate resource handlers,
//! and managing WebSocket connections for event streaming.

use crate::applications::StasisAppRegistry;
use crate::error::{AriErrorKind, AriResult};
use crate::models::AriError;
use crate::websocket::WebSocketSessionManager;
use dashmap::DashMap;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;

/// Authentication credentials for ARI access.
#[derive(Debug, Clone)]
pub enum AriAuth {
    /// HTTP Basic authentication (username, password).
    Basic { username: String, password: String },
    /// API key passed as query parameter.
    ApiKey(String),
}

/// Configuration for the ARI server.
#[derive(Debug, Clone)]
pub struct AriConfig {
    /// Whether ARI is enabled.
    pub enabled: bool,
    /// Bind address (e.g. "0.0.0.0:8088").
    pub bind_address: String,
    /// Allowed origins for CORS.
    pub allowed_origins: Vec<String>,
    /// Authentication mode: "basic" or "api_key" or "both".
    pub auth_mode: String,
    /// Configured users with their credentials.
    pub users: Vec<AriUser>,
    /// Pretty-print JSON responses.
    pub pretty_print: bool,
}

impl Default for AriConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            bind_address: "0.0.0.0:8088".to_string(),
            allowed_origins: vec!["*".to_string()],
            auth_mode: "basic".to_string(),
            users: Vec::new(),
            pretty_print: false,
        }
    }
}

/// An ARI user account.
#[derive(Debug, Clone)]
pub struct AriUser {
    pub username: String,
    pub password: String,
    /// If true, this user is read-only.
    pub read_only: bool,
}

/// HTTP method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Options,
    Patch,
}

impl std::fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Get => write!(f, "GET"),
            Self::Post => write!(f, "POST"),
            Self::Put => write!(f, "PUT"),
            Self::Delete => write!(f, "DELETE"),
            Self::Options => write!(f, "OPTIONS"),
            Self::Patch => write!(f, "PATCH"),
        }
    }
}

/// A parsed ARI HTTP request.
#[derive(Debug, Clone)]
pub struct AriRequest {
    /// HTTP method.
    pub method: HttpMethod,
    /// Request URI path (e.g. "/ari/channels/12345").
    pub path: String,
    /// Path segments split on '/'.
    pub path_segments: Vec<String>,
    /// Query parameters.
    pub query_params: std::collections::HashMap<String, Vec<String>>,
    /// Request body (JSON bytes).
    pub body: Option<bytes::Bytes>,
    /// Authenticated username, if any.
    pub username: Option<String>,
}

impl AriRequest {
    /// Get a single query parameter value.
    pub fn query_param(&self, name: &str) -> Option<&str> {
        self.query_params
            .get(name)
            .and_then(|v| v.first())
            .map(|s| s.as_str())
    }

    /// Get all values for a query parameter (for multi-value params).
    pub fn query_params_multi(&self, name: &str) -> Vec<&str> {
        self.query_params
            .get(name)
            .map(|v| v.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default()
    }

    /// Parse the JSON body into a typed request structure.
    pub fn parse_body<T: serde::de::DeserializeOwned>(&self) -> AriResult<T> {
        let body = self
            .body
            .as_ref()
            .ok_or_else(|| AriErrorKind::BadRequest("missing request body".into()))?;
        serde_json::from_slice(body)
            .map_err(|e| AriErrorKind::BadRequest(format!("invalid JSON: {}", e)))
    }

    /// Parse an optional JSON body. Returns Ok(None) if no body is present.
    pub fn parse_body_optional<T: serde::de::DeserializeOwned>(&self) -> AriResult<Option<T>> {
        match &self.body {
            Some(body) if !body.is_empty() => {
                let val = serde_json::from_slice(body)
                    .map_err(|e| AriErrorKind::BadRequest(format!("invalid JSON: {}", e)))?;
                Ok(Some(val))
            }
            _ => Ok(None),
        }
    }

    /// Extract a path variable by its segment index (after splitting on '/').
    /// For example, in "/ari/channels/{channelId}", the channelId is at index 2.
    pub fn path_var(&self, index: usize) -> Option<&str> {
        self.path_segments.get(index).map(|s| s.as_str())
    }
}

/// An ARI HTTP response.
#[derive(Debug, Clone)]
pub struct AriResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response body (JSON).
    pub body: Option<Vec<u8>>,
    /// Content type (default: application/json).
    pub content_type: String,
}

impl AriResponse {
    /// Create a 200 OK response with a JSON body.
    pub fn ok<T: Serialize>(value: &T) -> Self {
        let body = serde_json::to_vec(value).unwrap_or_default();
        Self {
            status: 200,
            body: Some(body),
            content_type: "application/json".into(),
        }
    }

    /// Create a 204 No Content response.
    pub fn no_content() -> Self {
        Self {
            status: 204,
            body: None,
            content_type: "application/json".into(),
        }
    }

    /// Create an error response.
    pub fn error(kind: &AriErrorKind) -> Self {
        let body = serde_json::to_vec(&kind.to_ari_error()).unwrap_or_default();
        Self {
            status: kind.status_code(),
            body: Some(body),
            content_type: "application/json".into(),
        }
    }

    /// Create a response with a specific status code and JSON body.
    pub fn with_status<T: Serialize>(status: u16, value: &T) -> Self {
        let body = serde_json::to_vec(value).unwrap_or_default();
        Self {
            status,
            body: Some(body),
            content_type: "application/json".into(),
        }
    }
}

/// REST handler tree node, mirroring Asterisk's stasis_rest_handlers.
///
/// Each node has a path segment, optional callbacks per HTTP method,
/// and child nodes for sub-paths.
pub struct RestHandler {
    /// Path segment this handler matches (e.g. "channels", "{channelId}").
    pub path_segment: String,
    /// Whether this segment is a path variable (starts with '{').
    pub is_wildcard: bool,
    /// Handler callbacks by HTTP method.
    pub callbacks: DashMap<HttpMethod, Arc<dyn Fn(&AriRequest, &AriServer) -> AriResponse + Send + Sync>>,
    /// Child handlers.
    pub children: RwLock<Vec<Arc<RestHandler>>>,
}

impl RestHandler {
    /// Create a new handler node for a path segment.
    pub fn new(path_segment: impl Into<String>) -> Self {
        let seg: String = path_segment.into();
        let is_wildcard = seg.starts_with('{') && seg.ends_with('}');
        Self {
            path_segment: seg,
            is_wildcard,
            callbacks: DashMap::new(),
            children: RwLock::new(Vec::new()),
        }
    }

    /// Register a callback for a specific HTTP method.
    pub fn on(
        self,
        method: HttpMethod,
        callback: impl Fn(&AriRequest, &AriServer) -> AriResponse + Send + Sync + 'static,
    ) -> Self {
        self.callbacks.insert(method, Arc::new(callback));
        self
    }

    /// Add a child handler.
    pub fn child(self, child: Arc<RestHandler>) -> Self {
        self.children.write().push(child);
        self
    }
}

impl std::fmt::Debug for RestHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RestHandler")
            .field("path_segment", &self.path_segment)
            .field("is_wildcard", &self.is_wildcard)
            .field("num_children", &self.children.read().len())
            .finish()
    }
}

/// The main ARI server, holding the route tree, auth config, and shared state.
pub struct AriServer {
    /// Server configuration.
    pub config: AriConfig,
    /// Root route handler (matches "ari").
    pub root_handler: Arc<RestHandler>,
    /// WebSocket session manager.
    pub websocket_sessions: Arc<WebSocketSessionManager>,
    /// Stasis application registry.
    pub app_registry: Arc<StasisAppRegistry>,
    /// Global variables (for /asterisk/variable).
    pub global_variables: DashMap<String, String>,
}

impl AriServer {
    /// Create a new ARI server with the given configuration.
    pub fn new(config: AriConfig) -> Self {
        let root = Arc::new(RestHandler::new("ari"));
        let ws_manager = Arc::new(WebSocketSessionManager::new());
        let app_registry = Arc::new(StasisAppRegistry::new(ws_manager.clone()));

        let server = Self {
            config,
            root_handler: root,
            websocket_sessions: ws_manager,
            app_registry,
            global_variables: DashMap::new(),
        };

        server
    }

    /// Install all built-in ARI resource routes.
    pub fn install_routes(&mut self) {
        let routes = crate::routes::build_route_tree();
        self.root_handler = routes;
    }

    /// Authenticate an incoming request.
    ///
    /// Returns Ok(username) on success, Err on failure.
    pub fn authenticate(&self, auth: &AriAuth) -> AriResult<String> {
        match auth {
            AriAuth::Basic { username, password } => {
                for user in &self.config.users {
                    if user.username == *username && user.password == *password {
                        return Ok(username.clone());
                    }
                }
                Err(AriErrorKind::Unauthorized)
            }
            AriAuth::ApiKey(key) => {
                // API keys are matched against configured user passwords
                for user in &self.config.users {
                    if user.password == *key {
                        return Ok(user.username.clone());
                    }
                }
                Err(AriErrorKind::Unauthorized)
            }
        }
    }

    /// Route a request through the handler tree and invoke the matched handler.
    pub fn handle_request(&self, request: &AriRequest) -> AriResponse {
        // Walk the path segments to find the matching handler
        let segments = &request.path_segments;
        if segments.is_empty() {
            return AriResponse::error(&AriErrorKind::NotFound("empty path".into()));
        }

        // The first segment should be "ari"
        if segments.first().map(|s| s.as_str()) != Some("ari") {
            return AriResponse::error(&AriErrorKind::NotFound(
                format!("unknown path: {}", request.path),
            ));
        }

        // Walk remaining segments
        let mut current = self.root_handler.clone();
        for seg in segments.iter().skip(1) {
            let next = {
                let children = current.children.read();
                children.iter().find(|child| {
                    child.path_segment == *seg || child.is_wildcard
                }).cloned()
            };
            match next {
                Some(handler) => {
                    current = handler;
                }
                None => {
                    return AriResponse::error(&AriErrorKind::NotFound(
                        format!("no handler for path: {}", request.path),
                    ));
                }
            }
        }

        // Look up the callback for this method
        let callback = current.callbacks.get(&request.method).map(|cb| cb.clone());
        match callback {
            Some(callback) => callback(request, self),
            None => {
                // Method not allowed
                AriResponse {
                    status: 405,
                    body: Some(
                        serde_json::to_vec(&AriError {
                            message: format!("method {} not allowed", request.method),
                        })
                        .unwrap_or_default(),
                    ),
                    content_type: "application/json".into(),
                }
            }
        }
    }

    /// Get the Swagger API docs listing.
    pub fn get_api_docs(&self) -> AriResponse {
        let docs = ApiDocsListing {
            api_version: "2.0.0".into(),
            swagger_version: "1.1".into(),
            base_path: format!("http://{}/ari", self.config.bind_address),
            apis: vec![
                ApiDocEntry { path: "/api-docs/asterisk.json".into(), description: "Asterisk resources".into() },
                ApiDocEntry { path: "/api-docs/endpoints.json".into(), description: "Endpoint resources".into() },
                ApiDocEntry { path: "/api-docs/channels.json".into(), description: "Channel resources".into() },
                ApiDocEntry { path: "/api-docs/bridges.json".into(), description: "Bridge resources".into() },
                ApiDocEntry { path: "/api-docs/recordings.json".into(), description: "Recording resources".into() },
                ApiDocEntry { path: "/api-docs/sounds.json".into(), description: "Sound resources".into() },
                ApiDocEntry { path: "/api-docs/playbacks.json".into(), description: "Playback control resources".into() },
                ApiDocEntry { path: "/api-docs/deviceStates.json".into(), description: "Device state resources".into() },
                ApiDocEntry { path: "/api-docs/mailboxes.json".into(), description: "Mailboxes resources".into() },
                ApiDocEntry { path: "/api-docs/events.json".into(), description: "WebSocket resource".into() },
                ApiDocEntry { path: "/api-docs/applications.json".into(), description: "Stasis application resources".into() },
            ],
        };
        AriResponse::ok(&docs)
    }
}

impl std::fmt::Debug for AriServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AriServer")
            .field("config", &self.config)
            .field("root_handler", &self.root_handler)
            .finish()
    }
}

/// Swagger API docs listing.
#[derive(Debug, Serialize, Deserialize)]
struct ApiDocsListing {
    #[serde(rename = "apiVersion")]
    api_version: String,
    #[serde(rename = "swaggerVersion")]
    swagger_version: String,
    #[serde(rename = "basePath")]
    base_path: String,
    apis: Vec<ApiDocEntry>,
}

/// Single entry in the API docs listing.
#[derive(Debug, Serialize, Deserialize)]
struct ApiDocEntry {
    path: String,
    description: String,
}
