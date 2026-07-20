//! Account-pool scheduler with Sub2API-like observable behavior.
//!
//! Independent implementation: do not copy Sub2API (LGPL) source.
//! Behavior is driven by documented knobs and parity tests.

mod scoring;
mod select;
mod types;

#[allow(unused_imports)]
pub use scoring::{lottery_weights, score_among_peers, score_candidate, QUOTA_SNAPSHOT_STALE_SECS};
pub use select::select_account;
#[allow(unused_imports)]
pub use select::sticky_eligible;
pub use types::*;
