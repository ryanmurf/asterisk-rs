//! SIP SUBSCRIBE/NOTIFY event framework (port of res_pjsip_pubsub.c).
//!
//! Implements RFC 3265 event notification framework: handles incoming
//! SUBSCRIBE requests, manages subscription state and expiration, and
//! generates NOTIFY messages when resource state changes.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use tracing::info;
use uuid::Uuid;

use crate::parser::{
    extract_tag, extract_uri, header_names, RequestLine, SipHeader, SipMessage, SipMethod, SipUri,
    StartLine, StatusLine,
};

// ---------------------------------------------------------------------------
// Subscription state
// ---------------------------------------------------------------------------

/// Subscription state per RFC 3265.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionState {
    /// Subscription has been created but not yet confirmed.
    Pending,
    /// Active subscription, notifications will be sent.
    Active,
    /// Subscription is being terminated.
    Terminated,
}

impl SubscriptionState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Active => "active",
            Self::Terminated => "terminated",
        }
    }
}

// ---------------------------------------------------------------------------
// Subscription
// ---------------------------------------------------------------------------

/// An individual SIP event subscription.
#[derive(Debug, Clone)]
pub struct Subscription {
    /// Unique subscription ID (also used as the dialog identifier).
    pub id: String,
    /// Event package name (e.g. "presence", "message-summary", "dialog").
    pub event_type: String,
    /// URI of the subscriber (From header).
    pub subscriber: String,
    /// Resource URI being watched (the request URI / To header).
    pub resource: String,
    /// Granted expiration in seconds.
    pub expiration: u32,
    /// When the subscription was created or last refreshed.
    pub created_at: Instant,
    /// Current subscription state.
    pub state: SubscriptionState,
    /// Call-ID for this subscription dialog.
    pub call_id: String,
    /// Local tag for the dialog.
    pub local_tag: String,
    /// Remote tag for the dialog.
    pub remote_tag: String,
    /// CSeq number for outgoing NOTIFYs.
    pub notify_cseq: u32,
    /// The remote Contact URI (for sending NOTIFYs).
    pub remote_contact: String,
    /// Accept header value from the SUBSCRIBE.
    pub accept: String,
}

impl Subscription {
    /// True when the subscription has expired.
    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed() >= Duration::from_secs(self.expiration as u64)
    }

    /// Remaining seconds until expiration.
    pub fn remaining_seconds(&self) -> u32 {
        let elapsed = self.created_at.elapsed().as_secs() as u32;
        self.expiration.saturating_sub(elapsed)
    }
}

// ---------------------------------------------------------------------------
// SubscriptionHandler trait
// ---------------------------------------------------------------------------

/// Trait for event-package handlers (presence, MWI, dialog-state, etc.).
///
/// Implementors generate the body for NOTIFY messages and decide whether
/// to accept subscriptions.
pub trait SubscriptionHandler: Send + Sync {
    /// Event name this handler serves (e.g. "presence", "message-summary").
    fn event_name(&self) -> &str;

    /// Default Accept content type.
    fn default_accept(&self) -> &str;

    /// Called when a new SUBSCRIBE arrives. Return `true` to accept.
    fn on_subscribe(&self, subscription: &Subscription) -> bool;

    /// Generate the NOTIFY body for the current state of a resource.
    /// Returns `(content_type, body)`.
    fn get_notify_body(&self, subscription: &Subscription) -> Option<(String, String)>;

    /// Called when a subscription is terminated.
    fn on_terminated(&self, _subscription: &Subscription) {}
}

// ---------------------------------------------------------------------------
// PubSub engine
// ---------------------------------------------------------------------------

/// Manages all SIP event subscriptions.
#[derive(Debug)]
pub struct PubSub {
    /// Active subscriptions keyed by subscription ID.
    subscriptions: RwLock<HashMap<String, Subscription>>,
    /// Default expiration for subscriptions (seconds).
    pub default_expiration: u32,
    /// Minimum expiration.
    pub min_expiration: u32,
    /// Maximum expiration.
    pub max_expiration: u32,
}

impl PubSub {
    pub fn new() -> Self {
        Self {
            subscriptions: RwLock::new(HashMap::new()),
            default_expiration: 3600,
            min_expiration: 60,
            max_expiration: 86400,
        }
    }

    /// Handle an incoming SUBSCRIBE request.
    ///
    /// `handler` is the event-package handler for the requested event type.
    /// Returns a tuple of (response_to_subscribe, optional_initial_notify).
    pub fn handle_subscribe(
        &self,
        request: &SipMessage,
        handler: &dyn SubscriptionHandler,
    ) -> (SipMessage, Option<SipMessage>) {
        if request.method() != Some(SipMethod::Subscribe) {
            return (self.make_error(request, 405, "Method Not Allowed"), None);
        }

        // Extract Event header.
        let event = match request.get_header("Event") {
            Some(e) => e.to_string(),
            None => return (self.make_error(request, 489, "Bad Event"), None),
        };

        if !event.starts_with(handler.event_name()) {
            return (self.make_error(request, 489, "Bad Event"), None);
        }

        // Determine expiration.
        let requested_expires: u32 = request
            .get_header(header_names::EXPIRES)
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(self.default_expiration);

        // Zero means unsubscribe.
        let is_unsubscribe = requested_expires == 0;
        let expiration = if is_unsubscribe {
            0
        } else {
            requested_expires
                .max(self.min_expiration)
                .min(self.max_expiration)
        };

        let from_hdr = request.from_header().unwrap_or("").to_string();
        let to_hdr = request.to_header().unwrap_or("").to_string();
        let subscriber = extract_uri(&from_hdr).unwrap_or_default();
        let resource = extract_uri(&to_hdr).unwrap_or_default();
        let call_id = request.call_id().unwrap_or("").to_string();
        let remote_tag = extract_tag(&from_hdr).unwrap_or_default();
        let local_tag = Uuid::new_v4().to_string()[..8].to_string();
        let remote_contact = request
            .get_header(header_names::CONTACT)
            .and_then(extract_uri)
            .unwrap_or_default();

        let accept = request
            .get_header("Accept")
            .unwrap_or(handler.default_accept())
            .to_string();

        // Check for refresh of existing subscription (same Call-ID + remote tag).
        let existing_id = {
            let subs = self.subscriptions.read();
            subs.values()
                .find(|s| s.call_id == call_id && s.remote_tag == remote_tag)
                .map(|s| s.id.clone())
        };

        let sub_id = existing_id.unwrap_or_else(|| Uuid::new_v4().to_string());

        if is_unsubscribe {
            // Terminate the subscription.
            let mut subs = self.subscriptions.write();
            if let Some(sub) = subs.get_mut(&sub_id) {
                sub.state = SubscriptionState::Terminated;
                sub.expiration = 0;
                handler.on_terminated(sub);
            }
            subs.remove(&sub_id);
            let response = self.build_subscribe_response(request, 200, "OK", 0, &local_tag);
            let notify = self.build_notify_terminated(request, &sub_id, &call_id, &local_tag, &remote_tag, &remote_contact, handler);
            return (response, Some(notify));
        }

        let subscription = Subscription {
            id: sub_id.clone(),
            event_type: handler.event_name().to_string(),
            subscriber,
            resource,
            expiration,
            created_at: Instant::now(),
            state: SubscriptionState::Active,
            call_id: call_id.clone(),
            local_tag: local_tag.clone(),
            remote_tag: remote_tag.clone(),
            notify_cseq: 0,
            remote_contact: remote_contact.clone(),
            accept,
        };

        // Ask handler whether to accept.
        if !handler.on_subscribe(&subscription) {
            return (self.make_error(request, 403, "Forbidden"), None);
        }

        // Store subscription.
        self.subscriptions.write().insert(sub_id.clone(), subscription.clone());

        // Build 200 OK for SUBSCRIBE.
        let response = self.build_subscribe_response(request, 200, "OK", expiration, &local_tag);

        // Build initial NOTIFY.
        let notify = self.build_notify(&sub_id, handler);

        info!(
            event = %event,
            subscriber = %subscription.subscriber,
            expires = expiration,
            "Subscription accepted"
        );

        (response, notify)
    }

    /// Build a NOTIFY message for a subscription.
    pub fn build_notify(
        &self,
        sub_id: &str,
        handler: &dyn SubscriptionHandler,
    ) -> Option<SipMessage> {
        let mut subs = self.subscriptions.write();
        let sub = subs.get_mut(sub_id)?;
        sub.notify_cseq += 1;
        let cseq = sub.notify_cseq;

        let (content_type, body) = handler.get_notify_body(sub)?;

        let branch = format!(
            "z9hG4bK{}",
            &Uuid::new_v4().to_string().replace('-', "")[..16]
        );

        let target_uri = SipUri::parse(&sub.remote_contact).ok().unwrap_or(SipUri {
            scheme: "sip".to_string(),
            user: None,
            password: None,
            host: "localhost".to_string(),
            port: Some(5060),
            parameters: Default::default(),
            headers: Default::default(),
        });

        let sub_state_value = format!(
            "{};expires={}",
            sub.state.as_str(),
            sub.remaining_seconds()
        );

        let headers = vec![
            SipHeader {
                name: header_names::VIA.to_string(),
                value: format!("SIP/2.0/UDP placeholder;branch={}", branch),
            },
            SipHeader {
                name: header_names::MAX_FORWARDS.to_string(),
                value: "70".to_string(),
            },
            SipHeader {
                name: header_names::FROM.to_string(),
                value: format!("<{}>;tag={}", sub.resource, sub.local_tag),
            },
            SipHeader {
                name: header_names::TO.to_string(),
                value: format!("<{}>;tag={}", sub.subscriber, sub.remote_tag),
            },
            SipHeader {
                name: header_names::CALL_ID.to_string(),
                value: sub.call_id.clone(),
            },
            SipHeader {
                name: header_names::CSEQ.to_string(),
                value: format!("{} NOTIFY", cseq),
            },
            SipHeader {
                name: header_names::CONTACT.to_string(),
                value: format!("<{}>", sub.resource),
            },
            SipHeader {
                name: "Event".to_string(),
                value: sub.event_type.clone(),
            },
            SipHeader {
                name: "Subscription-State".to_string(),
                value: sub_state_value,
            },
            SipHeader {
                name: header_names::CONTENT_TYPE.to_string(),
                value: content_type,
            },
            SipHeader {
                name: header_names::CONTENT_LENGTH.to_string(),
                value: body.len().to_string(),
            },
        ];

        Some(SipMessage {
            start_line: StartLine::Request(RequestLine {
                method: SipMethod::Notify,
                uri: target_uri,
                version: "SIP/2.0".to_string(),
            }),
            headers,
            body,
        })
    }

    /// Notify all subscribers for a given event type and resource.
    /// Returns a list of NOTIFY messages to send.
    pub fn notify_resource(
        &self,
        event_type: &str,
        resource: &str,
        handler: &dyn SubscriptionHandler,
    ) -> Vec<SipMessage> {
        let ids: Vec<String> = {
            let subs = self.subscriptions.read();
            subs.values()
                .filter(|s| {
                    s.event_type == event_type
                        && s.resource == resource
                        && s.state == SubscriptionState::Active
                })
                .map(|s| s.id.clone())
                .collect()
        };

        ids.iter()
            .filter_map(|id| self.build_notify(id, handler))
            .collect()
    }

    /// Remove expired subscriptions. Returns the number removed.
    pub fn purge_expired(&self) -> usize {
        let mut subs = self.subscriptions.write();
        let before = subs.len();
        subs.retain(|_, s| !s.is_expired());
        before - subs.len()
    }

    /// Get all active subscriptions.
    pub fn get_subscriptions(&self) -> Vec<Subscription> {
        self.subscriptions.read().values().cloned().collect()
    }

    // ---- helpers ----------------------------------------------------------

    fn build_subscribe_response(
        &self,
        request: &SipMessage,
        code: u16,
        reason: &str,
        expires: u32,
        local_tag: &str,
    ) -> SipMessage {
        let mut response = request
            .create_response(code, reason)
            .unwrap_or_else(|_| self.make_error(request, 500, "Internal Server Error"));

        // Add To tag.
        for h in &mut response.headers {
            if h.name.eq_ignore_ascii_case(header_names::TO) && !h.value.contains("tag=") {
                h.value = format!("{};tag={}", h.value, local_tag);
            }
        }

        // Add Expires header.
        response.headers.push(SipHeader {
            name: header_names::EXPIRES.to_string(),
            value: expires.to_string(),
        });

        response
    }

    fn build_notify_terminated(
        &self,
        _request: &SipMessage,
        _sub_id: &str,
        call_id: &str,
        local_tag: &str,
        remote_tag: &str,
        remote_contact: &str,
        handler: &dyn SubscriptionHandler,
    ) -> SipMessage {
        let branch = format!(
            "z9hG4bK{}",
            &Uuid::new_v4().to_string().replace('-', "")[..16]
        );

        let target_uri = SipUri::parse(remote_contact).ok().unwrap_or(SipUri {
            scheme: "sip".to_string(),
            user: None,
            password: None,
            host: "localhost".to_string(),
            port: Some(5060),
            parameters: Default::default(),
            headers: Default::default(),
        });

        let headers = vec![
            SipHeader {
                name: header_names::VIA.to_string(),
                value: format!("SIP/2.0/UDP placeholder;branch={}", branch),
            },
            SipHeader {
                name: header_names::MAX_FORWARDS.to_string(),
                value: "70".to_string(),
            },
            SipHeader {
                name: header_names::FROM.to_string(),
                value: format!("<placeholder>;tag={}", local_tag),
            },
            SipHeader {
                name: header_names::TO.to_string(),
                value: format!("<placeholder>;tag={}", remote_tag),
            },
            SipHeader {
                name: header_names::CALL_ID.to_string(),
                value: call_id.to_string(),
            },
            SipHeader {
                name: header_names::CSEQ.to_string(),
                value: "1 NOTIFY".to_string(),
            },
            SipHeader {
                name: "Event".to_string(),
                value: handler.event_name().to_string(),
            },
            SipHeader {
                name: "Subscription-State".to_string(),
                value: "terminated;reason=deactivated".to_string(),
            },
            SipHeader {
                name: header_names::CONTENT_LENGTH.to_string(),
                value: "0".to_string(),
            },
        ];

        SipMessage {
            start_line: StartLine::Request(RequestLine {
                method: SipMethod::Notify,
                uri: target_uri,
                version: "SIP/2.0".to_string(),
            }),
            headers,
            body: String::new(),
        }
    }

    fn make_error(&self, request: &SipMessage, code: u16, reason: &str) -> SipMessage {
        request
            .create_response(code, reason)
            .unwrap_or_else(|_| SipMessage {
                start_line: StartLine::Response(StatusLine {
                    version: "SIP/2.0".to_string(),
                    status_code: code,
                    reason_phrase: reason.to_string(),
                }),
                headers: Vec::new(),
                body: String::new(),
            })
    }
}

impl Default for PubSub {
    fn default() -> Self {
        Self::new()
    }
}
