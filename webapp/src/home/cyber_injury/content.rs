//! Browser-safe copy resolved by the brand catalog.

use serde::{Deserialize, Serialize};

/// Campaign copy and asset URLs carried through the home-page view.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CyberInjuryContent {
    pub eyebrow: String,
    pub hero_lines: [String; 4],
    pub hero_note: String,
    pub fee_heading: [String; 3],
    pub fee_body: String,
    pub fee_note: String,
    pub calculation_note: String,
    pub discovery_heading: [String; 2],
    pub discovery_body: String,
    pub discovery_cards: Vec<[String; 3]>,
    pub discovery_note: String,
    pub assessment_heading: [String; 3],
    pub assessment_body: String,
    pub assessment_note: String,
    pub recent_result: String,
    pub deadline_result: String,
    pub evidence_tips: [String; 4],
    pub medical_tips: [String; 2],
    pub deadline_tip: String,
    pub result_note: String,
    pub closing_lines: [String; 2],
    pub footer_note: String,
    pub hero_src: String,
    pub ad_src: String,
}
