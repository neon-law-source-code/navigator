//! The annual privacy offer, authored in the brand home catalog.
use serde::{Deserialize, Serialize};

/// Copy for the one-page data removal and privacy protection product.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivacyCopy {
    pub eyebrow: String,
    pub price: String,
    pub price_term: String,
    pub offer_note: String,
    pub gift_link: String,
    pub animation_label: String,
    pub animation_note: String,
    pub pause_label: String,
    pub benefits_heading: String,
    pub record_label: String,
    pub record_heading: String,
    pub record_body: String,
    pub gift_heading: String,
    pub gift_body: String,
    pub gift_cta: String,
    pub gift_card_label: String,
    pub gift_card_term: String,
    pub closing_heading: String,
    pub benefits: Vec<[String; 2]>,
}
