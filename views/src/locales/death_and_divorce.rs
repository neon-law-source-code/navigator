//! Copy for the Death & Divorce home page.

use serde::{Deserialize, Serialize};

/// The practice areas, video placeholder, and closing reassurance shown on
/// the Death & Divorce home page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeathAndDivorceCopy {
    pub eyebrow: String,
    pub statement_heading: String,
    pub statement_body: String,
    pub practices: Vec<[String; 2]>,
    pub video_label: String,
    pub video_body: String,
    pub process_label: String,
    pub process_heading: String,
    pub steps: Vec<[String; 2]>,
    pub closing_heading: String,
    pub closing_body: String,
}
