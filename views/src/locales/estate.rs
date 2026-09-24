//! Copy for the lifetime estate-planning page.
use serde::{Deserialize, Serialize};

/// Vesta's one-page offer, authored in the brand's home catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EstateCopy {
    pub eyebrow: String,
    pub process_link: String,
    pub plan_label: String,
    pub price: String,
    pub price_term: String,
    pub features: Vec<String>,
    pub fee_note: String,
    pub video_label: String,
    pub video_src: String,
    pub process_label: String,
    pub process_heading: String,
    pub steps: Vec<[String; 2]>,
    pub record_label: String,
    pub record_price: String,
    pub record_unit: String,
    pub record_status: String,
    pub record_heading: String,
    pub record_body: String,
    pub record_note: String,
    pub closing_heading: String,
    pub closing_body: String,
}
