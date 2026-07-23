use super::types::{CandidateAccount, PoolSchedulerConfig, ScoreWeights};

/// Snapshots older than this are treated as neutral for quota headroom.
pub const QUOTA_SNAPSHOT_STALE_SECS: i64 = 8 * 3600;

/// Higher is better. Factors are normalized to ~[0, 1] then weighted.
/// Independent implementation of Sub2API-observable scoring contracts.
#[allow(dead_code)]
pub fn score_candidate(
    candidate: &CandidateAccount,
    config: &PoolSchedulerConfig,
    now_unix: i64,
) -> f64 {
    score_among_peers(candidate, std::slice::from_ref(candidate), config, now_unix)
}

/// Score using peer set for min-max factors (priority / ttft / reset).
pub fn score_among_peers(
    candidate: &CandidateAccount,
    peers: &[CandidateAccount],
    config: &PoolSchedulerConfig,
    now_unix: i64,
) -> f64 {
    let weights = effective_weights(config);
    let priority_factor = priority_factor(candidate, peers);
    let load_factor = {
        let limit = candidate.concurrency_limit.max(1) as f64;
        1.0 - (candidate.active_leases as f64 / limit).clamp(0.0, 1.0)
    };
    let queue_factor = if candidate.active_leases >= candidate.concurrency_limit.max(1) {
        0.0
    } else {
        let limit = candidate.concurrency_limit.max(1) as f64;
        1.0 - (candidate.active_leases as f64 / limit).clamp(0.0, 1.0)
    };
    let error_factor = 1.0 - candidate.error_rate_ewma.clamp(0.0, 1.0);
    let ttft_factor = ttft_factor(candidate, peers);
    let reset_factor = reset_factor(candidate, peers, &weights, now_unix);
    let quota_factor = quota_headroom_factor(candidate, &weights, now_unix);
    let upstream_cost_factor = upstream_cost_factor(candidate, peers, &weights);

    weights.priority * priority_factor
        + weights.load * load_factor
        + weights.queue * queue_factor
        + weights.error_rate * error_factor
        + weights.ttft * ttft_factor
        + weights.reset * reset_factor
        + weights.quota_headroom * quota_factor
        + weights.upstream_cost * upstream_cost_factor
}

fn effective_weights(config: &PoolSchedulerConfig) -> ScoreWeights {
    let mut weights = config.score_weights.clone();
    if config.prefer_soonest_reset && weights.reset <= 0.0 {
        weights.reset = 0.5;
    }
    weights
}

fn clamp01(value: f64) -> f64 {
    value.clamp(0.0, 1.0)
}

/// Higher priority → higher factor (min-max within peers).
fn priority_factor(candidate: &CandidateAccount, peers: &[CandidateAccount]) -> f64 {
    let mut min_p = candidate.priority;
    let mut max_p = candidate.priority;
    for peer in peers {
        min_p = min_p.min(peer.priority);
        max_p = max_p.max(peer.priority);
    }
    if max_p > min_p {
        (candidate.priority - min_p) as f64 / (max_p - min_p) as f64
    } else {
        1.0
    }
}

fn ttft_factor(candidate: &CandidateAccount, peers: &[CandidateAccount]) -> f64 {
    let samples: Vec<f64> = peers
        .iter()
        .filter(|p| p.ttft_ewma_ms > 0.0)
        .map(|p| p.ttft_ewma_ms)
        .collect();
    if samples.is_empty() || candidate.ttft_ewma_ms <= 0.0 {
        return 0.5;
    }
    let min_t = samples.iter().copied().fold(f64::INFINITY, f64::min);
    let max_t = samples.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if max_t > min_t {
        1.0 - clamp01((candidate.ttft_ewma_ms - min_t) / (max_t - min_t))
    } else {
        1.0
    }
}

/// Sooner future reset → higher factor among peers that have a future window.
fn reset_factor(
    candidate: &CandidateAccount,
    peers: &[CandidateAccount],
    weights: &ScoreWeights,
    now_unix: i64,
) -> f64 {
    if weights.reset <= 0.0 {
        return 0.0;
    }
    let remainings: Vec<f64> = peers
        .iter()
        .filter_map(|p| {
            p.session_reset_at
                .filter(|at| *at > now_unix)
                .map(|at| (at - now_unix) as f64)
        })
        .collect();
    if remainings.is_empty() {
        return 0.0;
    }
    let Some(reset_at) = candidate.session_reset_at.filter(|at| *at > now_unix) else {
        return 0.0;
    };
    let remaining = (reset_at - now_unix) as f64;
    let min_r = remainings.iter().copied().fold(f64::INFINITY, f64::min);
    let max_r = remainings.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if max_r > min_r {
        1.0 - clamp01((remaining - min_r) / (max_r - min_r))
    } else {
        1.0
    }
}

/// Known remaining fraction; unknown → 0.5; stale snapshot → 0.5.
pub fn quota_headroom_factor(
    candidate: &CandidateAccount,
    weights: &ScoreWeights,
    now_unix: i64,
) -> f64 {
    if weights.quota_headroom <= 0.0 {
        return 0.0;
    }
    if let Some(fetched_at) = candidate.quota_fetched_at {
        if now_unix.saturating_sub(fetched_at) > QUOTA_SNAPSHOT_STALE_SECS {
            return 0.5;
        }
    }
    match candidate.quota_remaining {
        Some(value) => clamp01(value),
        None => 0.5,
    }
}

/// Lower upstream cost rate → higher factor (min-max among peers).
fn upstream_cost_factor(
    candidate: &CandidateAccount,
    peers: &[CandidateAccount],
    weights: &ScoreWeights,
) -> f64 {
    if weights.upstream_cost <= 0.0 {
        return 0.0;
    }
    let rates: Vec<f64> = peers
        .iter()
        .map(|p| {
            let rate = p.upstream_cost_rate;
            if rate.is_finite() && rate > 0.0 {
                rate
            } else {
                1.0
            }
        })
        .collect();
    if rates.is_empty() {
        return 0.5;
    }
    let min_r = rates.iter().copied().fold(f64::INFINITY, f64::min);
    let max_r = rates.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let rate = if candidate.upstream_cost_rate.is_finite() && candidate.upstream_cost_rate > 0.0 {
        candidate.upstream_cost_rate
    } else {
        1.0
    };
    if max_r > min_r {
        1.0 - clamp01((rate - min_r) / (max_r - min_r))
    } else {
        0.5
    }
}

/// Soft sticky bonus applied in sticky-weighted mode.
pub fn sticky_soft_bonus(
    candidate: &CandidateAccount,
    config: &PoolSchedulerConfig,
    previous_response_binding: Option<&str>,
    session_binding: Option<&str>,
) -> f64 {
    if !config.sticky_weighted_enabled {
        return 0.0;
    }
    let mut bonus = 0.0;
    if previous_response_binding == Some(candidate.credential_id.as_str()) {
        bonus += config.score_weights.previous_response.max(0.0);
    }
    if session_binding == Some(candidate.credential_id.as_str()) {
        bonus += config.score_weights.session_sticky.max(0.0);
    }
    bonus
}

/// Lottery weight among top-K: (score - min_score) + 1, times member weight.
pub fn lottery_weights(scores: &[f64], member_weights: &[i64]) -> Vec<f64> {
    assert_eq!(scores.len(), member_weights.len());
    if scores.is_empty() {
        return Vec::new();
    }
    let min_score = scores.iter().copied().fold(f64::INFINITY, f64::min);
    scores
        .iter()
        .zip(member_weights.iter())
        .map(|(score, mw)| {
            let base = (score - min_score) + 1.0;
            let base = if !base.is_finite() || base <= 0.0 {
                1.0
            } else {
                base
            };
            base * (*mw).max(1) as f64
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::types::{CandidateAccount, PoolSchedulerConfig, ScheduleState};

    fn base(id: &str) -> CandidateAccount {
        CandidateAccount {
            credential_id: id.into(),
            weight: 1,
            priority: 0,
            enabled: true,
            healthy: true,
            schedule_state: ScheduleState::Ready,
            cooldown_until: None,
            active_leases: 0,
            concurrency_limit: 1,
            error_rate_ewma: 0.0,
            ttft_ewma_ms: 0.0,
            quota_remaining: Some(1.0),
            session_reset_at: None,
            quota_fetched_at: Some(0),
            upstream_cost_rate: 1.0,
            last_used_at: None,
        }
    }

    #[test]
    fn higher_priority_scores_higher() {
        let config = PoolSchedulerConfig::default();
        let mut low = base("a");
        low.priority = 1;
        let mut high = base("b");
        high.priority = 50;
        let peers = vec![low.clone(), high.clone()];
        assert!(
            score_among_peers(&high, &peers, &config, 0)
                > score_among_peers(&low, &peers, &config, 0)
        );
    }

    #[test]
    fn load_penalizes_busy_accounts() {
        let config = PoolSchedulerConfig::default();
        let idle = base("a");
        let mut busy = base("b");
        busy.active_leases = 3;
        busy.concurrency_limit = 3;
        let peers = vec![idle.clone(), busy.clone()];
        assert!(
            score_among_peers(&idle, &peers, &config, 0)
                > score_among_peers(&busy, &peers, &config, 0)
        );
    }

    #[test]
    fn error_rate_weight_changes_order() {
        let mut config = PoolSchedulerConfig::default();
        config.score_weights.error_rate = 2.0;
        let mut clean = base("a");
        clean.priority = 10;
        clean.error_rate_ewma = 0.0;
        let mut flaky = base("b");
        flaky.priority = 20;
        flaky.error_rate_ewma = 0.9;
        let peers = vec![clean.clone(), flaky.clone()];
        assert!(
            score_among_peers(&clean, &peers, &config, 0)
                > score_among_peers(&flaky, &peers, &config, 0)
        );
    }

    #[test]
    fn sooner_reset_scores_higher_when_reset_weight_on() {
        let mut config = PoolSchedulerConfig::default();
        config.score_weights.reset = 1.0;
        let now = 1_700_000_000;
        let mut soon = base("soon");
        soon.session_reset_at = Some(now + 600);
        let mut late = base("late");
        late.session_reset_at = Some(now + 7200);
        let peers = vec![soon.clone(), late.clone()];
        assert!(
            score_among_peers(&soon, &peers, &config, now)
                > score_among_peers(&late, &peers, &config, now)
        );
    }

    #[test]
    fn prefer_soonest_reset_enables_reset_without_weight() {
        let mut config = PoolSchedulerConfig::default();
        config.score_weights.reset = 0.0;
        config.prefer_soonest_reset = true;
        let now = 1_700_000_000;
        let mut soon = base("soon");
        soon.session_reset_at = Some(now + 100);
        let mut late = base("late");
        late.session_reset_at = Some(now + 10_000);
        let peers = vec![soon.clone(), late.clone()];
        assert!(
            score_among_peers(&soon, &peers, &config, now)
                > score_among_peers(&late, &peers, &config, now)
        );
    }

    #[test]
    fn higher_quota_headroom_scores_higher() {
        let mut config = PoolSchedulerConfig::default();
        config.score_weights.quota_headroom = 1.0;
        let mut rich = base("rich");
        rich.quota_remaining = Some(0.9);
        rich.quota_fetched_at = Some(1_700_000_000);
        let mut poor = base("poor");
        poor.quota_remaining = Some(0.1);
        poor.quota_fetched_at = Some(1_700_000_000);
        let peers = vec![rich.clone(), poor.clone()];
        assert!(
            score_among_peers(&rich, &peers, &config, 1_700_000_000)
                > score_among_peers(&poor, &peers, &config, 1_700_000_000)
        );
    }

    #[test]
    fn stale_quota_becomes_neutral() {
        let mut config = PoolSchedulerConfig::default();
        config.score_weights.quota_headroom = 1.0;
        let now = 1_700_000_000;
        let mut stale = base("stale");
        stale.quota_remaining = Some(0.99);
        stale.quota_fetched_at = Some(now - QUOTA_SNAPSHOT_STALE_SECS - 1);
        let mut fresh = base("fresh");
        fresh.quota_remaining = Some(0.2);
        fresh.quota_fetched_at = Some(now);
        // stale should not dominate just from high remaining
        let peers = vec![stale.clone(), fresh.clone()];
        let stale_score = score_among_peers(&stale, &peers, &config, now);
        let fresh_score = score_among_peers(&fresh, &peers, &config, now);
        // with only quota weight differing effectively, stale=0.5 vs fresh=0.2 → stale still higher
        // but stale must equal neutral 0.5 factor: compare against known remaining 0.5
        let mut neutral = base("n");
        neutral.quota_remaining = None;
        neutral.quota_fetched_at = None;
        assert!(
            (quota_headroom_factor(&stale, &config.score_weights, now) - 0.5).abs() < 1e-9
        );
        let _ = (stale_score, fresh_score, neutral);
    }

    #[test]
    fn lottery_weights_shift_by_min_score() {
        let weights = lottery_weights(&[5.0, 3.0, 3.0], &[1, 1, 1]);
        assert!((weights[0] - 3.0).abs() < 1e-9); // (5-3)+1
        assert!((weights[1] - 1.0).abs() < 1e-9);
        assert!((weights[2] - 1.0).abs() < 1e-9);
    }
}
