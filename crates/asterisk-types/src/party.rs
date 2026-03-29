use serde::{Deserialize, Serialize};

/// Character set for party names, matching Q.SIG values from `AST_PARTY_CHAR_SET`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[repr(u8)]
pub enum PartyCharSet {
    Unknown = 0,
    #[default]
    Iso8859_1 = 1,
    Withdrawn = 2,
    Iso8859_2 = 3,
    Iso8859_3 = 4,
    Iso8859_4 = 5,
    Iso8859_5 = 6,
    Iso8859_7 = 7,
    Iso10646BmpString = 8,
    Iso10646Utf8String = 9,
}

/// Name information for a party in a call, corresponding to `ast_party_name`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PartyName {
    /// Subscriber name
    pub name: String,
    /// Character set the name is using
    pub char_set: PartyCharSet,
    /// Q.931 presentation-indicator
    pub presentation: i32,
    /// Whether the name information is valid/present
    pub valid: bool,
}

/// Number information for a party in a call, corresponding to `ast_party_number`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PartyNumber {
    /// Subscriber phone number
    pub number: String,
    /// Q.931 Type-Of-Number and Numbering-Plan encoded fields
    pub plan: i32,
    /// Q.931 presentation-indicator and screening-indicator
    pub presentation: i32,
    /// Whether the number information is valid/present
    pub valid: bool,
}

/// Subaddress information, corresponding to `ast_party_subaddress`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PartySubaddress {
    /// Subaddress string (may be BCD-encoded hex for user-specified type)
    pub subaddress: String,
    /// Q.931 subaddress type: 0=NSAP, 2=user_specified
    pub subaddress_type: i32,
    /// True if odd number of address signals
    pub odd_even_indicator: bool,
    /// Whether the subaddress information is valid/present
    pub valid: bool,
}

/// Party identification, corresponding to `ast_party_id`.
///
/// Combines name, number, and subaddress to identify an endpoint.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PartyId {
    /// Subscriber name
    pub name: PartyName,
    /// Subscriber phone number
    pub number: PartyNumber,
    /// Subscriber subaddress
    pub subaddress: PartySubaddress,
    /// User-set tag for associating extrinsic information
    pub tag: String,
}

/// Caller party information, corresponding to `ast_party_caller`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CallerId {
    /// Caller party ID
    pub id: PartyId,
    /// Automatic Number Identification (ANI)
    pub ani: PartyId,
    /// Private caller party ID
    pub priv_id: PartyId,
    /// ANI2 (Info Digits)
    pub ani2: i32,
}

/// Connected line/party information, corresponding to `ast_party_connected_line`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConnectedLine {
    /// Connected party ID
    pub id: PartyId,
    /// ANI (saved from caller)
    pub ani: PartyId,
    /// Private connected party ID
    pub priv_id: PartyId,
    /// ANI2 (Info Digits)
    pub ani2: i32,
    /// Source of the update
    pub source: i32,
}

/// Redirecting reason information, corresponding to `ast_party_redirecting_reason`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RedirectingReason {
    /// String value for the redirecting reason
    pub reason_str: String,
    /// Enum code for redirection reason
    pub code: i32,
}

/// Dialed/called party information, corresponding to `ast_party_dialed`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DialedParty {
    /// Dialed/called number
    pub number: String,
    /// Q.931 Type-Of-Number and Numbering-Plan
    pub number_plan: i32,
    /// Dialed/called subaddress
    pub subaddress: PartySubaddress,
    /// Transit Network Select
    pub transit_network_select: i32,
}

/// Redirecting line information (RDNIS), corresponding to `ast_party_redirecting`.
///
/// Contains information about where a call diversion or transfer was invoked.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Redirecting {
    /// Who originally redirected the call
    pub orig: PartyId,
    /// Who is redirecting the call
    pub from: PartyId,
    /// Call is redirecting to a new party
    pub to: PartyId,
    /// Private: who originally redirected
    pub priv_orig: PartyId,
    /// Private: who is redirecting
    pub priv_from: PartyId,
    /// Private: redirecting to
    pub priv_to: PartyId,
    /// Reason for the redirection
    pub reason: RedirectingReason,
    /// Reason for the redirection by the original party
    pub orig_reason: RedirectingReason,
    /// Number of times the call was redirected
    pub count: i32,
}

/// Number presentation constants matching Q.931 values.
pub mod presentation {
    /// Presentation allowed, number available
    pub const ALLOWED: i32 = 0x00;
    /// Presentation restricted
    pub const RESTRICTED: i32 = 0x20;
    /// Number not available
    pub const UNAVAILABLE: i32 = 0x43;
}
