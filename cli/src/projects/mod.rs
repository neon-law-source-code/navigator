//! `navigator site projects` — the Project workspace group.
//!
//! Project is the organizing noun of the whole product: a matter's Drive
//! ingest folder and its one source repository are both addressed by one
//! `projects.code`. The verbs that operate on that pair live here rather than
//! under `site`, because they are not driven by a live-site bearer token
//! alone — they resolve deployment-owned coordinates from [`cloud::workspace`]
//! and inspect the operator's own machine.

pub mod doctor;
pub mod drift;
pub mod repository;
pub mod surfaces;
