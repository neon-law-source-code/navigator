//! Notation rate catalog and quoted meter entries.
//!
//! These records explain a price before work begins: `$10/day` for library
//! access, `$5` for an unchanged signature send, and `$100+` for prepared or
//! revised work. They never create an invoice; the lawyer raises that agreed
//! invoice in Xero.

/// Stable rate codes used by the client-facing catalog and meter entries.
pub mod rate_code {
    pub const LIBRARY_ACCESS_DAY: &str = "library_access_day";
    pub const UNCHANGED_SIGNATURE_SEND: &str = "unchanged_signature_send";
    pub const PREPARED_OR_REVISED_NOTATION: &str = "prepared_or_revised_notation";
}

/// A fixed, integer-money rate. No floating point prices enter the billing
/// path, and a meter entry snapshots this amount when it is quoted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotationRate {
    pub code: String,
    pub label: String,
    pub unit: String,
    pub unit_amount_cents: i64,
    pub currency: String,
}

impl NotationRate {
    #[must_use]
    pub fn total_cents(&self, quantity: i64) -> i64 {
        self.unit_amount_cents.saturating_mul(quantity)
    }
}

/// A non-invoicing estimate from a rate and measured quantity.
#[must_use]
pub fn quote_total_cents(rate: &NotationRate, quantity: i64) -> i64 {
    rate.total_cents(quantity)
}

#[cfg(test)]
mod tests {
    use super::{quote_total_cents, rate_code, NotationRate};

    #[test]
    fn the_public_rates_are_exact_integer_money_quotes() {
        let access = NotationRate {
            code: rate_code::LIBRARY_ACCESS_DAY.into(),
            label: "Contract library access".into(),
            unit: "day".into(),
            unit_amount_cents: 1_000,
            currency: "USD".into(),
        };
        let signature = NotationRate {
            code: rate_code::UNCHANGED_SIGNATURE_SEND.into(),
            label: "Unchanged signature send".into(),
            unit: "send".into(),
            unit_amount_cents: 500,
            currency: "USD".into(),
        };
        let notation = NotationRate {
            code: rate_code::PREPARED_OR_REVISED_NOTATION.into(),
            label: "Prepared or revised Notation".into(),
            unit: "notation".into(),
            unit_amount_cents: 10_000,
            currency: "USD".into(),
        };
        assert_eq!(quote_total_cents(&access, 1), 1_000);
        assert_eq!(quote_total_cents(&signature, 1), 500);
        assert_eq!(quote_total_cents(&notation, 1), 10_000);
    }
}
