//! Copy for the Daybridge divorce-law home page.

use serde::{Deserialize, Serialize};

/// The focused offer, service commitments, and short engagement flow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaybridgeCopy {
    pub eyebrow: String,
    pub fee_label: String,
    pub fee_price: String,
    pub fee_unit: String,
    pub fee_body: String,
    pub fee_note: String,
    pub response_heading: String,
    pub response_body: String,
    pub motions_heading: String,
    pub motions_body: String,
    pub costs_heading: String,
    pub costs_body: String,
    pub process_label: String,
    pub process_heading: String,
    pub steps: Vec<[String; 2]>,
    pub closing_heading: String,
    pub closing_body: String,
}
