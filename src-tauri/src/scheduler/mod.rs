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
pub use select::{quota_blocks, sticky_concurrency_full, sticky_eligible};
pub use types::{
    BindingKind, CandidateAccount, PoolSchedulerConfig, RoutingMode, ScheduleState, SelectOutcome,
    SelectRequest, SelectionLayer,
};
// Re-export optional types used by storage/proxy without requiring every consumer to name them.
#[allow(unused_imports)]
pub use types::{FallbackSelectionMode, ScoreWeights, StickyEscapeConfig};
