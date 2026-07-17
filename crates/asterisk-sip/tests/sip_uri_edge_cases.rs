//! Focused coverage for `SipUri::parse` error/edge branches that the two
//! happy-path unit tests in `parser/mod.rs` don't reach: scheme validation,
//! IPv6 hosts (with and without port), userinfo passwords, and the
//! parameter/header split. These run on the request-line and Contact/Route
//! URIs of every inbound request.

use asterisk_sip::parser::SipUri;

#[test]
fn missing_scheme_is_rejected() {
    assert!(SipUri::parse("no-colon-here").is_err());
}

#[test]
fn unknown_scheme_is_rejected() {
    assert!(SipUri::parse("http://example.com").is_err());
}

#[test]
fn scheme_is_case_insensitive() {
    assert_eq!(SipUri::parse("SIP:a@b").unwrap().scheme, "sip");
    assert_eq!(SipUri::parse("SIPS:a@b").unwrap().scheme, "sips");
    assert_eq!(SipUri::parse("TeL:+15551234").unwrap().scheme, "tel");
}

#[test]
fn ipv6_host_with_port() {
    let u = SipUri::parse("sip:[2001:db8::1]:5060").unwrap();
    assert_eq!(u.host, "2001:db8::1");
    assert_eq!(u.port, Some(5060));
    assert!(u.is_ipv6_host());
    assert_eq!(u.host_display(), "[2001:db8::1]");
}

#[test]
fn ipv6_host_without_port() {
    let u = SipUri::parse("sip:[::1]").unwrap();
    assert_eq!(u.host, "::1");
    assert_eq!(u.port, None);
}

#[test]
fn userinfo_with_password() {
    let u = SipUri::parse("sip:alice:s3cr3t@atlanta.example.com").unwrap();
    assert_eq!(u.user.as_deref(), Some("alice"));
    assert_eq!(u.password.as_deref(), Some("s3cr3t"));
    assert_eq!(u.host, "atlanta.example.com");
}

#[test]
fn parameters_and_headers_split() {
    let u = SipUri::parse("sip:a@b.com;transport=tcp;lr?Subject=hi&Priority=urgent").unwrap();
    assert_eq!(u.transport(), Some("tcp"));
    // Valueless parameter is present with a None value.
    assert!(u.parameters.contains_key("lr"));
    assert_eq!(u.parameters.get("lr"), Some(&None));
    // Header after '?' is parsed.
    assert_eq!(u.headers.get("Subject").map(String::as_str), Some("hi"));
    assert_eq!(u.headers.get("Priority").map(String::as_str), Some("urgent"));
    // The host must not absorb the params/headers.
    assert_eq!(u.host, "b.com");
}

#[test]
fn display_round_trip_preserves_ipv6_brackets() {
    let u = SipUri::parse("sip:bob@[2001:db8::2]:5061").unwrap();
    let rendered = u.to_string();
    let reparsed = SipUri::parse(&rendered).unwrap();
    assert_eq!(reparsed.host, "2001:db8::2");
    assert_eq!(reparsed.port, Some(5061));
}
