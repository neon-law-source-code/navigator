//! Shared CLI helpers that both the `navigator` binary and its tests compile.
//!
//! Catalog seeding lives here so `cli/tests/import.rs` exercises the same
//! crate llvm-cov measures, rather than a second `#[path]` copy that the
//! coverage report ignores.

pub mod import;
