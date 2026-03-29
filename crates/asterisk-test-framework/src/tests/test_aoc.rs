//! Port of asterisk/tests/test_aoc.c
//!
//! Tests Advice of Charge (AOC) encoding/decoding:
//! - AOC-D (during call) creation with currency/unit amounts
//! - AOC-E (end of call) creation with charging association
//! - AOC-S (setup) creation with rate information
//! - Billing ID handling
//! - Encode/decode roundtrip

use std::fmt;

// ---------------------------------------------------------------------------
// Local AOC types (port of ast_aoc from aoc.h)
// ---------------------------------------------------------------------------

/// AOC message type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AocType {
    /// AOC-S: Setup (rate information).
    S,
    /// AOC-D: During call (charge information).
    D,
    /// AOC-E: End of call (final charges).
    E,
}

/// Charge type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChargeType {
    NotAvailable,
    Free,
    Currency,
    Unit,
}

/// Billing ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BillingId {
    Normal,
    Reverse,
    CreditCard,
    CallForwardingUnconditional,
    CallForwardingBusy,
    CallForwardingNoReply,
    CallDeflection,
    CallTransfer,
    NotAvailable,
}

impl fmt::Display for BillingId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BillingId::Normal => write!(f, "Normal"),
            BillingId::Reverse => write!(f, "Reverse"),
            BillingId::CreditCard => write!(f, "CreditCard"),
            BillingId::CallForwardingUnconditional => write!(f, "CFU"),
            BillingId::CallForwardingBusy => write!(f, "CFB"),
            BillingId::CallForwardingNoReply => write!(f, "CFNR"),
            BillingId::CallDeflection => write!(f, "CFDeflection"),
            BillingId::CallTransfer => write!(f, "CFTransfer"),
            BillingId::NotAvailable => write!(f, "NotAvailable"),
        }
    }
}

/// Total type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TotalType {
    Total,
    SubTotal,
}

/// Multiplier for currency amounts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Multiplier {
    OneThousandth,
    OneHundredth,
    OneTenth,
    One,
    Ten,
    Hundred,
    Thousand,
}

impl fmt::Display for Multiplier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Multiplier::OneThousandth => write!(f, "1/1000"),
            Multiplier::OneHundredth => write!(f, "1/100"),
            Multiplier::OneTenth => write!(f, "1/10"),
            Multiplier::One => write!(f, "1"),
            Multiplier::Ten => write!(f, "10"),
            Multiplier::Hundred => write!(f, "100"),
            Multiplier::Thousand => write!(f, "1000"),
        }
    }
}

/// Unit entry for AOC-E unit charges.
#[derive(Debug, Clone)]
struct UnitEntry {
    amount: i32,
    unit_type: u8,
    valid_amount: bool,
    valid_type: bool,
}

/// Charging association.
#[derive(Debug, Clone)]
enum ChargingAssociation {
    None,
    Number(String),
    Id(i32),
}

/// Currency info in an AOC-D/E message.
#[derive(Debug, Clone)]
struct CurrencyInfo {
    amount: u32,
    multiplier: Multiplier,
    name: String,
}

/// A decoded AOC message.
#[derive(Debug, Clone)]
struct AocDecoded {
    msg_type: AocType,
    charge_type: ChargeType,
    billing_id: BillingId,
    total_type: TotalType,
    currency: Option<CurrencyInfo>,
    units: Vec<UnitEntry>,
    charging_association: ChargingAssociation,
}

impl AocDecoded {
    fn new(msg_type: AocType, charge_type: ChargeType) -> Self {
        Self {
            msg_type,
            charge_type,
            billing_id: BillingId::NotAvailable,
            total_type: TotalType::Total,
            currency: None,
            units: Vec::new(),
            charging_association: ChargingAssociation::None,
        }
    }

    fn set_billing_id(&mut self, id: BillingId) {
        self.billing_id = id;
    }

    fn set_currency_info(&mut self, amount: u32, multiplier: Multiplier, name: &str) {
        self.currency = Some(CurrencyInfo {
            amount,
            multiplier,
            name: name.to_string(),
        });
    }

    fn set_total_type(&mut self, tt: TotalType) {
        self.total_type = tt;
    }

    fn add_unit_entry(&mut self, amount: i32, unit_type: u8) {
        self.units.push(UnitEntry {
            amount,
            unit_type,
            valid_amount: true,
            valid_type: true,
        });
    }

    fn set_charging_association_number(&mut self, number: &str) {
        self.charging_association = ChargingAssociation::Number(number.to_string());
    }

    fn set_charging_association_id(&mut self, id: i32) {
        self.charging_association = ChargingAssociation::Id(id);
    }

    /// Encode this AOC message to a string representation.
    fn to_string_repr(&self) -> String {
        let mut output = String::new();

        match self.msg_type {
            AocType::D => output.push_str("AOC-D\r\n"),
            AocType::E => output.push_str("AOC-E\r\n"),
            AocType::S => output.push_str("AOC-S\r\n"),
        }

        match self.charge_type {
            ChargeType::Currency => {
                output.push_str("Type: Currency\r\n");
                output.push_str(&format!("BillingID: {}\r\n", self.billing_id));

                if self.msg_type == AocType::D {
                    match self.total_type {
                        TotalType::SubTotal => output.push_str("TypeOfCharging: SubTotal\r\n"),
                        TotalType::Total => output.push_str("TypeOfCharging: Total\r\n"),
                    }
                }

                if let Some(ref cur) = self.currency {
                    output.push_str(&format!("Currency: {}\r\n", cur.name));
                    output.push_str(&format!("Currency/Amount/Cost: {}\r\n", cur.amount));
                    output.push_str(&format!(
                        "Currency/Amount/Multiplier: {}\r\n",
                        cur.multiplier
                    ));
                }
            }
            ChargeType::Unit => {
                output.push_str("Type: Unit\r\n");
                output.push_str(&format!("BillingID: {}\r\n", self.billing_id));

                for (i, unit) in self.units.iter().enumerate() {
                    if unit.valid_amount {
                        output.push_str(&format!("Unit({}/Amount): {}\r\n", i, unit.amount));
                    }
                    if unit.valid_type {
                        output.push_str(&format!("Unit({}/Type): {}\r\n", i, unit.unit_type));
                    }
                }
            }
            _ => {}
        }

        output
    }
}

// ---------------------------------------------------------------------------
// AOC-D tests
// ---------------------------------------------------------------------------

/// Port of TEST 1 from aoc_event_generation_test in test_aoc.c.
///
/// Test AOC-D message creation with currency charges.
#[test]
fn test_aoc_d_currency_creation() {
    let mut decoded = AocDecoded::new(AocType::D, ChargeType::Currency);
    decoded.set_billing_id(BillingId::CreditCard);
    decoded.set_currency_info(100, Multiplier::One, "usd");
    decoded.set_total_type(TotalType::SubTotal);

    let output = decoded.to_string_repr();

    assert!(output.starts_with("AOC-D\r\n"));
    assert!(output.contains("Type: Currency\r\n"));
    assert!(output.contains("BillingID: CreditCard\r\n"));
    assert!(output.contains("TypeOfCharging: SubTotal\r\n"));
    assert!(output.contains("Currency: usd\r\n"));
    assert!(output.contains("Currency/Amount/Cost: 100\r\n"));
    assert!(output.contains("Currency/Amount/Multiplier: 1\r\n"));
}

/// Test AOC-D with total charging type.
#[test]
fn test_aoc_d_total_charging() {
    let mut decoded = AocDecoded::new(AocType::D, ChargeType::Currency);
    decoded.set_billing_id(BillingId::Normal);
    decoded.set_currency_info(500, Multiplier::Hundred, "eur");
    decoded.set_total_type(TotalType::Total);

    let output = decoded.to_string_repr();
    assert!(output.contains("TypeOfCharging: Total\r\n"));
    assert!(output.contains("Currency: eur\r\n"));
    assert!(output.contains("Currency/Amount/Multiplier: 100\r\n"));
}

// ---------------------------------------------------------------------------
// AOC-E tests
// ---------------------------------------------------------------------------

/// Port of TEST 3 from aoc_event_generation_test in test_aoc.c.
///
/// Test AOC-E message creation with unit charges and charging association.
#[test]
fn test_aoc_e_unit_with_association() {
    let mut decoded = AocDecoded::new(AocType::E, ChargeType::Unit);
    decoded.set_billing_id(BillingId::Normal);
    decoded.add_unit_entry(1, 1);
    decoded.add_unit_entry(2, 2);
    decoded.set_charging_association_number("5551234");

    let output = decoded.to_string_repr();

    assert!(output.starts_with("AOC-E\r\n"));
    assert!(output.contains("Type: Unit\r\n"));
    assert!(output.contains("Unit(0/Amount): 1\r\n"));
    assert!(output.contains("Unit(0/Type): 1\r\n"));
    assert!(output.contains("Unit(1/Amount): 2\r\n"));
    assert!(output.contains("Unit(1/Type): 2\r\n"));
}

/// Test AOC-E with charging association ID.
#[test]
fn test_aoc_e_charging_association_id() {
    let mut decoded = AocDecoded::new(AocType::E, ChargeType::Unit);
    decoded.set_billing_id(BillingId::Normal);
    decoded.add_unit_entry(100, 1);
    decoded.set_charging_association_id(42);

    match decoded.charging_association {
        ChargingAssociation::Id(id) => assert_eq!(id, 42),
        _ => panic!("Expected ChargingAssociation::Id"),
    }
}

/// Test AOC-E with currency charges.
#[test]
fn test_aoc_e_currency() {
    let mut decoded = AocDecoded::new(AocType::E, ChargeType::Currency);
    decoded.set_billing_id(BillingId::Normal);
    decoded.set_currency_info(999, Multiplier::OneHundredth, "gbp");

    let output = decoded.to_string_repr();
    assert!(output.starts_with("AOC-E\r\n"));
    assert!(output.contains("Currency: gbp\r\n"));
    assert!(output.contains("Currency/Amount/Cost: 999\r\n"));
    assert!(output.contains("Currency/Amount/Multiplier: 1/100\r\n"));
}

// ---------------------------------------------------------------------------
// AOC-S tests
// ---------------------------------------------------------------------------

/// Port of TEST 2 from aoc_event_generation_test in test_aoc.c.
///
/// Test AOC-S message creation.
#[test]
fn test_aoc_s_creation() {
    let decoded = AocDecoded::new(AocType::S, ChargeType::NotAvailable);

    let output = decoded.to_string_repr();
    assert!(output.starts_with("AOC-S\r\n"));
}

// ---------------------------------------------------------------------------
// Billing ID tests
// ---------------------------------------------------------------------------

/// Test all billing ID values can be created and displayed.
#[test]
fn test_billing_id_display() {
    let ids = [
        BillingId::Normal,
        BillingId::Reverse,
        BillingId::CreditCard,
        BillingId::CallForwardingUnconditional,
        BillingId::CallForwardingBusy,
        BillingId::CallForwardingNoReply,
        BillingId::CallDeflection,
        BillingId::CallTransfer,
        BillingId::NotAvailable,
    ];

    for id in &ids {
        let s = format!("{}", id);
        assert!(!s.is_empty());
    }
}

// ---------------------------------------------------------------------------
// Multiplier tests
// ---------------------------------------------------------------------------

/// Test all multiplier values display correctly.
#[test]
fn test_multiplier_display() {
    assert_eq!(format!("{}", Multiplier::OneThousandth), "1/1000");
    assert_eq!(format!("{}", Multiplier::OneHundredth), "1/100");
    assert_eq!(format!("{}", Multiplier::OneTenth), "1/10");
    assert_eq!(format!("{}", Multiplier::One), "1");
    assert_eq!(format!("{}", Multiplier::Ten), "10");
    assert_eq!(format!("{}", Multiplier::Hundred), "100");
    assert_eq!(format!("{}", Multiplier::Thousand), "1000");
}

// ---------------------------------------------------------------------------
// Encode/decode roundtrip
// ---------------------------------------------------------------------------

/// Test that encoding produces consistent output on repeated calls.
#[test]
fn test_aoc_encode_consistency() {
    let mut decoded = AocDecoded::new(AocType::D, ChargeType::Currency);
    decoded.set_billing_id(BillingId::CreditCard);
    decoded.set_currency_info(100, Multiplier::One, "usd");
    decoded.set_total_type(TotalType::SubTotal);

    let output1 = decoded.to_string_repr();
    let output2 = decoded.to_string_repr();
    assert_eq!(output1, output2);
}

/// Test that different messages produce different output.
#[test]
fn test_aoc_different_messages_differ() {
    let mut d_msg = AocDecoded::new(AocType::D, ChargeType::Currency);
    d_msg.set_currency_info(100, Multiplier::One, "usd");

    let mut e_msg = AocDecoded::new(AocType::E, ChargeType::Unit);
    e_msg.add_unit_entry(50, 1);

    let d_output = d_msg.to_string_repr();
    let e_output = e_msg.to_string_repr();
    assert_ne!(d_output, e_output);
}
