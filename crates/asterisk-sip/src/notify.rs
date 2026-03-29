//! Arbitrary SIP NOTIFY generation (port of res_pjsip_notify.c).
//!
//! Sends configured NOTIFY messages such as check-sync, reboot,
//! or custom event notifications. Templates are loaded from
//! configuration and can be sent to endpoints or arbitrary URIs.

use std::collections::HashMap;

use parking_lot::RwLock;
use uuid::Uuid;

use crate::parser::{
    header_names, RequestLine, SipHeader, SipMessage, SipMethod, SipUri, StartLine,
};

// ---------------------------------------------------------------------------
// Notify template
// ---------------------------------------------------------------------------

/// A key-value pair that represents either a SIP header or a body line
/// in a NOTIFY template.
#[derive(Debug, Clone)]
pub struct NotifyOptionItem {
    pub name: String,
    pub value: String,
}

/// A configured NOTIFY template (e.g. "check-sync", "reboot").
#[derive(Debug, Clone)]
pub struct NotifyTemplate {
    /// Template name (matches a section in pjsip_notify.conf).
    pub name: String,
    /// Header and content items. Items named "Content" form the body;
    /// all others are added as SIP headers.
    pub items: Vec<NotifyOptionItem>,
}

impl NotifyTemplate {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            items: Vec::new(),
        }
    }

    /// Add a header or content item.
    pub fn add_item(&mut self, name: &str, value: &str) {
        self.items.push(NotifyOptionItem {
            name: name.to_string(),
            value: value.to_string(),
        });
    }

    /// Get all custom headers (items whose name is not "Content").
    pub fn headers(&self) -> Vec<&NotifyOptionItem> {
        self.items
            .iter()
            .filter(|i| !i.name.eq_ignore_ascii_case("Content"))
            .collect()
    }

    /// Get the body content (items named "Content" concatenated).
    pub fn body(&self) -> String {
        self.items
            .iter()
            .filter(|i| i.name.eq_ignore_ascii_case("Content"))
            .map(|i| i.value.as_str())
            .collect::<Vec<_>>()
            .join("\r\n")
    }

    /// Get the Content-Type from the template headers.
    pub fn content_type(&self) -> Option<&str> {
        self.items
            .iter()
            .find(|i| i.name.eq_ignore_ascii_case("Content-Type"))
            .map(|i| i.value.as_str())
    }
}

// ---------------------------------------------------------------------------
// Template registry
// ---------------------------------------------------------------------------

/// Registry of NOTIFY templates.
#[derive(Debug, Default)]
pub struct NotifyConfig {
    templates: RwLock<HashMap<String, NotifyTemplate>>,
}

impl NotifyConfig {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a NOTIFY template.
    pub fn add_template(&self, template: NotifyTemplate) {
        self.templates
            .write()
            .insert(template.name.clone(), template);
    }

    /// Retrieve a template by name.
    pub fn get_template(&self, name: &str) -> Option<NotifyTemplate> {
        self.templates.read().get(name).cloned()
    }

    /// List all template names.
    pub fn list_templates(&self) -> Vec<String> {
        self.templates.read().keys().cloned().collect()
    }

    /// Load built-in templates (check-sync, reboot).
    pub fn load_defaults(&self) {
        // Polycom check-sync.
        let mut check_sync = NotifyTemplate::new("check-sync");
        check_sync.add_item("Event", "check-sync");
        check_sync.add_item("Content-Type", "application/simple-message-summary");
        check_sync.add_item("Content", "Messages-Waiting: no");
        self.add_template(check_sync);

        // Cisco/Linksys reboot.
        let mut reboot = NotifyTemplate::new("reboot");
        reboot.add_item("Event", "reboot_now");
        self.add_template(reboot);

        // Yealink action-uri (auto-provision).
        let mut autoprov = NotifyTemplate::new("yealink-autoprovision");
        autoprov.add_item("Event", "check-sync;reboot=false");
        self.add_template(autoprov);
    }
}

// ---------------------------------------------------------------------------
// Reserved headers that cannot be overridden
// ---------------------------------------------------------------------------

const RESERVED_HEADERS: &[&str] = &[
    "Call-ID",
    "Contact",
    "CSeq",
    "To",
    "From",
    "Record-Route",
    "Route",
    "Via",
];

fn is_reserved(name: &str) -> bool {
    RESERVED_HEADERS
        .iter()
        .any(|h| name.eq_ignore_ascii_case(h))
}

// ---------------------------------------------------------------------------
// NOTIFY builder
// ---------------------------------------------------------------------------

/// Build a NOTIFY request using a template.
pub fn build_notify_from_template(
    template: &NotifyTemplate,
    to_uri: &str,
    from_uri: &str,
    call_id: &str,
    from_tag: &str,
    remote_tag: &str,
    cseq: u32,
) -> SipMessage {
    let branch = format!(
        "z9hG4bK{}",
        &Uuid::new_v4().to_string().replace('-', "")[..16]
    );

    let target_uri = SipUri::parse(to_uri).unwrap_or_else(|_| SipUri {
        scheme: "sip".to_string(),
        user: None,
        password: None,
        host: "localhost".to_string(),
        port: Some(5060),
        parameters: Default::default(),
        headers: Default::default(),
    });

    let body = template.body();

    let mut headers = vec![
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
            value: format!("<{}>;tag={}", from_uri, from_tag),
        },
        SipHeader {
            name: header_names::TO.to_string(),
            value: if remote_tag.is_empty() {
                format!("<{}>", to_uri)
            } else {
                format!("<{}>;tag={}", to_uri, remote_tag)
            },
        },
        SipHeader {
            name: header_names::CALL_ID.to_string(),
            value: call_id.to_string(),
        },
        SipHeader {
            name: header_names::CSEQ.to_string(),
            value: format!("{} NOTIFY", cseq),
        },
    ];

    // Add custom headers from the template (skip reserved ones).
    for item in template.headers() {
        if !is_reserved(&item.name) {
            headers.push(SipHeader {
                name: item.name.clone(),
                value: item.value.clone(),
            });
        }
    }

    // Content-Length.
    headers.push(SipHeader {
        name: header_names::CONTENT_LENGTH.to_string(),
        value: body.len().to_string(),
    });

    SipMessage {
        start_line: StartLine::Request(RequestLine {
            method: SipMethod::Notify,
            uri: target_uri,
            version: "SIP/2.0".to_string(),
        }),
        headers,
        body,
    }
}

/// Build a NOTIFY request from ad-hoc variables (no template).
pub fn build_notify_adhoc(
    to_uri: &str,
    from_uri: &str,
    variables: &[(String, String)],
) -> SipMessage {
    let call_id = format!("notify-{}", Uuid::new_v4());
    let from_tag = Uuid::new_v4().to_string()[..8].to_string();

    let mut template = NotifyTemplate::new("adhoc");
    for (name, value) in variables {
        template.add_item(name, value);
    }

    build_notify_from_template(
        &template,
        to_uri,
        from_uri,
        &call_id,
        &from_tag,
        "",
        1,
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_notify_template() {
        let mut tpl = NotifyTemplate::new("test");
        tpl.add_item("Event", "check-sync");
        tpl.add_item("Content-Type", "text/plain");
        tpl.add_item("Content", "line one");
        tpl.add_item("Content", "line two");

        assert_eq!(tpl.headers().len(), 2); // Event + Content-Type
        assert_eq!(tpl.body(), "line one\r\nline two");
        assert_eq!(tpl.content_type(), Some("text/plain"));
    }

    #[test]
    fn test_build_notify() {
        let config = NotifyConfig::new();
        config.load_defaults();

        let tpl = config.get_template("check-sync").unwrap();

        let msg = build_notify_from_template(
            &tpl,
            "sip:phone@10.0.0.1",
            "sip:asterisk@10.0.0.2",
            "call-123",
            "abc",
            "",
            1,
        );

        assert_eq!(msg.method(), Some(SipMethod::Notify));
        assert!(msg.get_header("Event").is_some());
    }

    #[test]
    fn test_reserved_headers() {
        assert!(is_reserved("Call-ID"));
        assert!(is_reserved("via"));
        assert!(!is_reserved("Event"));
        assert!(!is_reserved("Content-Type"));
    }
}
