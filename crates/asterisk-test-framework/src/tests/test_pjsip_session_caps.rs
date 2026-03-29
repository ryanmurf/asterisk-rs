//! Port of asterisk/tests/test_res_pjsip_session_caps.c
//!
//! Tests SIP session codec capability negotiation:
//! - Local/remote preference ordering
//! - First-only mode
//! - Joint capability computation
//! - Merge modes (outgoing only)
//! - Empty intersection handling

// ---------------------------------------------------------------------------
// Codec / format cap simulation
// ---------------------------------------------------------------------------

/// Simplified codec representation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Codec(String);

impl Codec {
    fn new(name: &str) -> Self {
        Self(name.to_string())
    }
}

impl std::fmt::Display for Codec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Preference for codec ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodecPref {
    Local,
    LocalFirst,
    LocalMerge,
    Remote,
    RemoteFirst,
    RemoteMerge,
}

fn parse_codecs(s: &str) -> Vec<Codec> {
    if s == "!all" || s == "nothing" {
        return Vec::new();
    }
    s.split(',')
        .map(|c| Codec::new(c.trim()))
        .collect()
}

/// Compute joint capabilities based on preference.
fn create_joint(
    local: &[Codec],
    remote: &[Codec],
    pref: CodecPref,
    is_outgoing: bool,
) -> Option<Vec<Codec>> {
    match pref {
        CodecPref::Local => {
            let joint: Vec<Codec> = local
                .iter()
                .filter(|c| remote.contains(c))
                .cloned()
                .collect();
            if joint.is_empty() {
                None
            } else {
                Some(joint)
            }
        }
        CodecPref::LocalFirst => {
            let first = local.iter().find(|c| remote.contains(c))?;
            Some(vec![first.clone()])
        }
        CodecPref::Remote => {
            let joint: Vec<Codec> = remote
                .iter()
                .filter(|c| local.contains(c))
                .cloned()
                .collect();
            if joint.is_empty() {
                None
            } else {
                Some(joint)
            }
        }
        CodecPref::RemoteFirst => {
            let first = remote.iter().find(|c| local.contains(c))?;
            Some(vec![first.clone()])
        }
        CodecPref::LocalMerge => {
            if !is_outgoing {
                return None; // Invalid for incoming.
            }
            // All local codecs (even those not in remote), preserving local order.
            Some(local.to_vec())
        }
        CodecPref::RemoteMerge => {
            if !is_outgoing {
                return None; // Invalid for incoming.
            }
            // Remote-ordered intersection, then remaining local codecs appended.
            let mut result: Vec<Codec> = remote
                .iter()
                .filter(|c| local.contains(c))
                .cloned()
                .collect();
            for c in local {
                if !result.contains(c) {
                    result.push(c.clone());
                }
            }
            if result.is_empty() {
                None
            } else {
                Some(result)
            }
        }
    }
}

fn codecs_to_string(codecs: &[Codec]) -> String {
    codecs
        .iter()
        .map(|c| c.0.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

fn run_test(
    local: &str,
    remote: &str,
    pref: CodecPref,
    is_outgoing: bool,
    expected: &str,
    should_pass: bool,
) {
    let local_codecs = parse_codecs(local);
    let remote_codecs = parse_codecs(remote);

    let result = create_joint(&local_codecs, &remote_codecs, pref, is_outgoing);

    if !should_pass {
        // We expect failure (None or mismatch).
        if let Some(joint) = result {
            let joint_str = codecs_to_string(&joint);
            assert_ne!(
                joint_str, expected,
                "Expected failure but got match: {}",
                joint_str
            );
        }
        return;
    }

    let joint = result.unwrap_or_default();
    let joint_str = if joint.is_empty() {
        "nothing".to_string()
    } else {
        codecs_to_string(&joint)
    };
    assert_eq!(
        joint_str, expected,
        "Mismatch: local={}, remote={}, pref={:?}, outgoing={}",
        local, remote, pref, is_outgoing
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Port of AST_TEST_DEFINE(low_level) from test_res_pjsip_session_caps.c.
///
/// Incoming tests.
#[test]
fn test_incoming_local_pref() {
    run_test("ulaw,alaw,g722", "g722,alaw,g729", CodecPref::Local, false, "alaw,g722", true);
}

#[test]
fn test_incoming_local_first() {
    run_test("ulaw,alaw,g722", "g722,alaw,g729", CodecPref::LocalFirst, false, "alaw", true);
}

#[test]
fn test_incoming_remote_pref() {
    run_test("ulaw,alaw,g722", "g722,alaw,g729", CodecPref::Remote, false, "g722,alaw", true);
}

#[test]
fn test_incoming_remote_first() {
    run_test("ulaw,alaw,g722", "g722,alaw,g729", CodecPref::RemoteFirst, false, "g722", true);
}

/// No intersection should fail.
#[test]
fn test_incoming_no_intersection() {
    let local = parse_codecs("ulaw,alaw,g722");
    let remote = parse_codecs("g729");
    let result = create_joint(&local, &remote, CodecPref::Local, false);
    assert!(result.is_none());
}

/// Merge modes are invalid for incoming.
#[test]
fn test_incoming_local_merge_invalid() {
    let local = parse_codecs("ulaw,alaw,g722");
    let remote = parse_codecs("g722,alaw,g729");
    let result = create_joint(&local, &remote, CodecPref::LocalMerge, false);
    assert!(result.is_none());
}

#[test]
fn test_incoming_remote_merge_invalid() {
    let local = parse_codecs("ulaw,alaw,g722");
    let remote = parse_codecs("g722,alaw,g729");
    let result = create_joint(&local, &remote, CodecPref::RemoteMerge, false);
    assert!(result.is_none());
}

/// Outgoing tests.
#[test]
fn test_outgoing_local_pref() {
    run_test("ulaw,alaw,g722", "g722,g729,alaw", CodecPref::Local, true, "alaw,g722", true);
}

#[test]
fn test_outgoing_local_first() {
    run_test("ulaw,alaw,g722", "g722,g729,alaw", CodecPref::LocalFirst, true, "alaw", true);
}

#[test]
fn test_outgoing_local_merge() {
    run_test("ulaw,alaw,g722", "g722,g729,alaw", CodecPref::LocalMerge, true, "ulaw,alaw,g722", true);
}

#[test]
fn test_outgoing_remote_pref() {
    run_test("ulaw,alaw,g722", "g722,g729,alaw", CodecPref::Remote, true, "g722,alaw", true);
}

#[test]
fn test_outgoing_remote_first() {
    run_test("ulaw,alaw,g722", "g722,g729,alaw", CodecPref::RemoteFirst, true, "g722", true);
}

#[test]
fn test_outgoing_remote_merge() {
    run_test("ulaw,alaw,g722", "g722,g729,alaw", CodecPref::RemoteMerge, true, "g722,alaw,ulaw", true);
}

/// Empty local set produces "nothing".
#[test]
fn test_outgoing_empty_local_remote_merge() {
    run_test("!all", "g722,g729,alaw", CodecPref::RemoteMerge, true, "nothing", true);
}
